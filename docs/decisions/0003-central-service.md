# 0003: Central service

Status: decided 2026-09-23, before any client integration, as #17 requires. Deployment waits for a Google Cloud project and the user's approval of the cost.

## Decision

- **Code:** a separate repository, [crate-digger-service](https://github.com/timini/crate-digger-service). Rust with axum, built as a container.
- **Runtime:** Google Cloud Run in its own Google Cloud project. It scales to zero when unused.
- **Data:** Firestore in native mode for the shared catalogue and contributions. A Cloud Storage bucket with uniform access holds private backup snapshots, because snapshots can exceed Firestore's 1 MiB document limit. Bigtable was rejected because it bills per node-hour even when idle; Cloud SQL was ruled out by the user.
- **Contract:** the `cd-protocol` crate in the app repository. The service depends on it by git tag. `crates/protocol/tests/v1` pins the JSON of every message, and both repositories run those tests.
- **Sign-in:** Google. The app runs the OAuth 2.0 loopback flow with PKCE in the system browser and keeps the refresh token in the OS keychain. Every request carries a Google ID token. The service checks its signature against Google's published keys, its audience (the app's OAuth client id), issuer and expiry, and uses the token's subject as the account id. There are no unauthenticated endpoints except a health check.

## Data model

| Collection | Key | Written by | Notes |
| --- | --- | --- | --- |
| `recordings` | service id | service | Consensus metadata, alternatives, known feature versions, references |
| `recording_keys` | fingerprint hash or `source:id` | service | Points a key at a recording; conflicts are kept, not merged |
| `contributions` | account + idempotency key | service on behalf of one account | Create-only: a retry finds the document and returns "duplicate"; nothing is overwritten |
| `features` | recording + feature version + account | service | Only the pinned model and its exact dimensions |
| `changes` | increasing sequence | service | Cursor-paged catalogue changes |
| `accounts/{sub}/backups` | backup id | service | Metadata only; the snapshot is in Cloud Storage under `accounts/{sub}/` |
| `quotas/{sub}` | day | service | Daily request and upload counts |

## Rules the service enforces

- Every request is validated with the `cd-protocol` rules: schema, unknown fields refused, text lengths, batch size, body size (1 MiB shared, 20 MiB backup), only the pinned model version with its exact dimensions, finite values.
- Contributions are untrusted. Consensus metadata is the value most distinct accounts agree on. Disagreeing values stay as alternatives, and a correction never deletes what it disagrees with.
- Backups are read, listed, restored and deleted only through the owner's account id, which is taken from the verified token and never from the request. Tests check that another account cannot see or touch them.
- Rate limits apply per account: a token bucket per instance, plus a daily count in Firestore.

## Privacy

Shared contributions carry only metadata, references, fingerprint hashes and embeddings. The protocol types have no field for local paths, ratings or credentials, and unknown fields are refused. Ratings leave the device only inside a private backup, which the user enables separately.

## Retention and access (draft, to publish before launch)

- **Shared catalogue:** contributions are kept for as long as the catalogue runs. Deleting an account removes the link between the account and its contributions, but the consensus data stays.
- **Backups:** kept until the user deletes them or the account. Deletion removes the Cloud Storage object at once.
- **Service logs:** hold request metadata but not bodies, and are kept for 30 days.

## Cost at pilot scale

- Cloud Run scales to zero, and pilot traffic sits inside the free tier.
- Firestore's free daily quota (50,000 reads, 20,000 writes) covers a pilot of a few DJs.
- Cloud Storage for backups costs cents a month.
- Artifact Registry charges a small amount for image storage.

Expected total: under $5 a month at pilot scale. Billing must be linked to the project, and a budget alert should be set at deployment.
