# Google Auth Roadmap

## Status

Today, local Google Docs and Google Sheets ingestion uses a Google service account. A user shares a document or sheet with the service account email, and memkit reads that content locally.

That flow is functional, but it is not the ideal long-term default for an open-source CLI because it depends on external Google credentials and can encourage awkward secret handling if documented casually.

## Product Direction

We should document and build toward two separate stories:

1. Local / open-source usage

- Users bring their own Google credentials.
- The preferred long-term direction is user-owned Google OAuth for desktop / CLI usage.
- In the short term, advanced users can still wire up their own Google service account locally if they understand the tradeoffs.

2. Hosted / paid usage

- Google ingestion can run through a memkit-managed backend.
- Backend-managed credentials and token storage fit better for a paid hosted feature than for the open-source local CLI.
- This avoids asking open-source users to trust a shared memkit-controlled Google credential.

## Documentation TODO

- Explain clearly that Google Docs / Sheets access is separate from `mk login` and separate from memkit cloud auth.
- Avoid presenting a shared memkit-owned service account as the default open-source setup.
- Document a bring-your-own-credentials path for local users.
- Document the security tradeoffs of service-account keys versus user OAuth.
- Add a hosted-ingestion section once the paid API path exists.

## Implementation TODO

- Evaluate replacing the local service-account flow with a true user OAuth desktop flow for Google Docs / Sheets.
- If service accounts remain supported locally, prefer `GOOGLE_APPLICATION_CREDENTIALS` pointing to a user-managed file over large inline JSON in `.env`.
- Keep Google provider credentials for memkit cloud login separate from Google Docs / Sheets ingestion credentials.

## Open Questions

- Should the local open-source path support both user OAuth and bring-your-own service-account keys, or only user OAuth?
- When hosted ingestion exists, which parts of indexing remain local versus move to the backend?
- What UX should explain the difference between local indexing credentials and memkit account login?
