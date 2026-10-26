# Pace project sharing

Durable upload/download of complete project snapshots with explicit optimistic
concurrency. This is **project sharing, not real-time co-editing**: there are no
live operations, presence, automatic merges, CRDTs, or last-writer-wins updates.
Work locally, download a revision, edit, and explicitly upload against that revision.

## Implementation sequence

1. Storage and revision API: SQLite persistence, unchanged project
   bytes, bounded requests, strong revision ETags, transactional writes, access control.
2. Authentication: provision bearer identities and enable the server binary.
   The bare router fails closed without an identity; the binary always uses auth.
3. Client integration: the `pace_app` browser editor uploads/downloads its existing
   draft format and offers explicit conflict decisions. The server also accepts
   core scenes through the same revision API. See the app's project-sharing guide.

## Supported project formats

Uploads are limited to **64 MiB**, including whitespace and embedded images,
matching the editor's project-file limit. Validation uses immutable dependencies:

- **PACE core scene v1** (`format_version: 1`): `Scene::from_json` from
  `pace_core` at `1aa2b36270b68ea6fc04c268c3d5661ec09790aa` checks shape, version,
  stable IDs, references, geometry and dialogue exits. Unknown fields are rejected.
- **Editor draft v1** (`format: "pace-editor-draft", version: 1`):
  `pace_editor_format::Project::from_json` from `pace_app` at
  `1dd8f4d8fcfb84849164a6041895dd15a1e9ef19` is the editor's own loader, extracted
  without schema changes. It rejects malformed structures, unsupported versions,
  unknown fields and duplicate IDs. Incomplete drafts and unresolved references
  remain editable, just as with local save/load; this does not certify playability.

The server stores/returns accepted UTF-8 bytes unchanged: no envelope, conversion,
rewritten IDs or asset extraction. Whitespace, field order and numeric spelling
are preserved. Embedded backgrounds travel inside the draft. Core scenes and
drafts remain distinct contracts; runtime export/conversion is not part of sharing.
Other JSON, runtime save slots and future versions are rejected. Previously stored
arbitrary JSON from the initial API remains downloadable, but new writes must use
a supported format. The transport never executes scripts or fetches asset URLs.
Rust 1.97.1+ is required by the pinned core dependency.

## Run the authenticated server

Generate independent, high-entropy tokens per user/device; use at least 32 random
bytes. The server config contains only SHA-256 digests, never plaintext tokens.
This is operator-provisioned bearer authentication, not a password/login service.
The following creates two local test identities with restricted file permissions:

```sh
umask 077
python3 - <<'PY'
import hashlib, json, pathlib, secrets
tokens = []
for principal in ("alice", "bob"):
    token = secrets.token_urlsafe(32)
    pathlib.Path(principal + ".token").write_text(token)
    tokens.append({"principal": principal, "sha256": hashlib.sha256(token.encode()).hexdigest()})
pathlib.Path("auth.json").write_text(json.dumps({"tokens": tokens}))
PY
PACE_AUTH_FILE=./auth.json PACE_DATABASE=./pace-projects.sqlite3 cargo run --locked
```

`PACE_AUTH_FILE` is mandatory and must contain 1–1024 entries. Principal IDs are
1–128 ASCII letters, digits, or `-_.@`. `PACE_DATABASE` defaults to
`pace-projects.sqlite3`; use a persistent, access-restricted data directory.
`PACE_BIND` defaults to `127.0.0.1:3000`. TLS must be terminated by a trusted
reverse proxy before exposing the service beyond localhost; bearer tokens confer
access to anyone holding them. Do not log authorization headers. There is no
browser login flow in this layer. Cross-origin browser access is disabled unless
`PACE_ALLOWED_ORIGINS` lists exact origins separated by commas, for example:

```sh
PACE_ALLOWED_ORIGINS=http://127.0.0.1:8080,http://localhost:8080 \
PACE_AUTH_FILE=./auth.json PACE_DATABASE=./pace-projects.sqlite3 cargo run --locked
```

Paths, credentials and wildcard origins are rejected. Allowed preflight requests
do not require authentication; actual project requests always do. The policy
allows Authorization, Content-Type and If-Match, and exposes ETag and Location.
Use a same-origin reverse proxy instead if preferred. CORS is a browser policy;
authorization still governs all callers.

For token rotation, add a new digest with the **same principal**, distribute that
token securely, remove the old digest, and restart. Multiple distinct digests may
map to the same principal; duplicate digests and malformed configs are rejected.
Principals remain stable across token rotation so ownership and membership survive.
Revocation takes effect for new requests after restart. Existing in-flight requests
may complete. Only operators can provision identities; owners may grant project
access to principal IDs, including IDs provisioned later. Recheck spelling before
granting access. Do not commit token files or operator config.

## Two-client HTTP walkthrough

Use a JSON file saved by the editor, or copy
`tests/fixtures/core-scene-v1.json` to `project.json` to exercise the canonical
core fixture. These commands use `curl` and `jq`, with the tokens created above:

```sh
BASE=http://127.0.0.1:3000
ALICE_TOKEN=$(cat alice.token)
BOB_TOKEN=$(cat bob.token)

# Alice creates and shares a private project with Bob.
ID=$(curl --fail-with-body -sS -X POST "$BASE/v1/projects" \
  -H "Authorization: Bearer $ALICE_TOKEN" -H 'Content-Type: application/json' \
  --data-binary @project.json | jq -r .id)
curl --fail-with-body -sS -X PUT "$BASE/v1/projects/$ID/members/bob" \
  -H "Authorization: Bearer $ALICE_TOKEN" -H 'Content-Type: application/json' \
  --data '{"role":"writer"}'

# Bob downloads exact bytes and the accompanying ETag, then edits bob.json locally.
curl --fail-with-body -sS -D bob.headers -o bob.json "$BASE/v1/projects/$ID" \
  -H "Authorization: Bearer $BOB_TOKEN"
ETAG=$(awk 'tolower($1)=="etag:" {gsub("\r", "", $2); print $2}' bob.headers)
curl --fail-with-body -sS -i -X PUT "$BASE/v1/projects/$ID" \
  -H "Authorization: Bearer $BOB_TOKEN" -H 'Content-Type: application/json' \
  -H "If-Match: $ETAG" --data-binary @bob.json

# Reusing the old ETag deliberately yields 412; no data is overwritten.
curl -sS -i -X PUT "$BASE/v1/projects/$ID" \
  -H "Authorization: Bearer $ALICE_TOKEN" -H 'Content-Type: application/json' \
  -H "If-Match: $ETAG" --data-binary @project.json
unset ALICE_TOKEN BOB_TOKEN
```

Clients must keep the downloaded body and its ETag together. Keep credentials in
the platform credential store; never put them in the scene document or a URL.
On a connection failure after uploading, the outcome may be unknown: download
the remote snapshot to inspect it before retrying. There is no automatic retry
or idempotency key for project creation.

## HTTP API

All project endpoints require a trusted authenticated principal. Project IDs are
server-generated UUIDs. Send uncompressed `Content-Type: application/json`.

| Request | Success | Behavior |
| --- | --- | --- |
| `POST /v1/projects` | `201`, `Location`, `ETag`, `{id,revision}` | Create a private project from raw JSON |
| `GET /v1/projects/{id}` | `200`, `ETag`, original bytes | Owner or member downloads snapshot |
| `PUT /v1/projects/{id}` + `If-Match: "1"` | `204`, new `ETag` | Owner/writer replaces snapshot atomically |
| `PUT /v1/projects/{id}/members/{principal}` + `{"role":"reader"}` or `{"role":"writer"}` | `204` | Owner grants/changes member access |
| `DELETE /v1/projects/{id}/members/{principal}` | `204` | Owner revokes access |
| `GET /health` | `200` | Liveness only |

Revision ETags are strong, quoted positive integers scoped to one project.
Every successful PUT increments the revision, including identical uploads.
PUT must target an existing project and supply exactly one strong ETag obtained
from that project; wildcard, weak, list, duplicate, or malformed `If-Match`
conditions are rejected with `400`. An absent condition returns `428`.
A stale condition returns `412 Precondition Failed`:

```json
{"error":"revision_conflict","message":"Project changed; download the latest version and reconcile before retrying","current_revision":2}
```

On conflict, retain local edits, download the current snapshot separately,
reconcile explicitly, and then submit with the newly downloaded ETag. Never
automatically retry the stale body using `current_revision`.
These conditional request semantics follow
[RFC 9110 §13.1.1](https://www.rfc-editor.org/rfc/rfc9110#section-13.1.1)
and [§15.5.13](https://www.rfc-editor.org/rfc/rfc9110#section-15.5.13),
with a deliberately restricted condition syntax to require an explicit revision.

Missing/invalid credentials return `401`; unknown and inaccessible projects
both return `404`. Readers attempting writes receive `403`. Invalid JSON/schema returns
`400`, unsupported media/encoding `415`, and oversized projects `413`.
Revision conflicts never change the stored bytes or revision.
ACL changes are owner-only, do not change the content ETag, and are serialized
with project writes. The owner cannot revoke their own access.

## Storage and validation

SQLite uses WAL journaling, foreign keys, full synchronization, and immediate
transactions for authorization/revision checks and updates. Use a persistent local
filesystem; multi-host deployments on network filesystems are not supported.
Keep the database and its WAL/SHM files together; use SQLite's backup API or stop
the server before copying the database. Only the latest snapshot is retained.

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Integration tests use real HTTP sockets and temporary SQLite files, including
independent database connections racing the same revision, two-client exchange,
process kill/restart persistence of contents/revisions/ACLs, denied access,
reader/writer/owner permissions, malformed/oversized (including chunked) payloads,
required preconditions, canonical core/draft fixtures, schema errors and CORS.
The app's browser suite starts this real server and verifies local save → upload
→ download → save equality for the complete three-room draft, including images,
as well as two-user conflicts, explicit reconciliation and denied access.
