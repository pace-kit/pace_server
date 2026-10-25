# Pace project sharing

Durable upload/download of complete project snapshots with explicit optimistic
concurrency. This is **project sharing, not real-time co-editing**: there are no
live operations, presence, automatic merges, CRDTs, or last-writer-wins updates.
Work locally, download a revision, edit, and explicitly upload against that revision.

## Implementation sequence

1. Storage and revision API (this layer): SQLite persistence, unchanged project
   bytes, bounded requests, strong revision ETags, transactional writes, access control.
2. Authentication: configure bearer identities, attach the trusted principal,
   and enable the server binary. The bare router fails closed without an identity.
3. Client integration: connect the local editor's serializer and expose conflicts
   to the user after the local core/editor workflow is available.

## Scene compatibility prerequisite

The referenced `pace_core` main at `6ccfd6e` has no scene schema/serializer;
`pace_app` main at `b195b90` is a demo, without project save/load. The server
therefore validates **JSON syntax only**, up to 1 MiB (including whitespace),
and stores/returns the original UTF-8 bytes. Unknown fields, whitespace, key order,
and numeric spelling are preserved. The examples/tests are transport fixtures,
not a proposed Pace scene format. Structurally invalid scenes that are valid JSON
cannot yet be rejected: integrate the core's canonical validator when available.
The transport does not execute scripts, dereference asset URLs, or unpack archives.
External assets are not uploaded by this API.

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
both return `404`. Readers attempting writes receive `403`. Invalid JSON returns
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
reopen persistence, malformed/oversized payloads, and required preconditions.
