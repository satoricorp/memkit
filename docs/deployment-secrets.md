# Deployment Secrets

## Goal

Keep local development simple while keeping Google service-account credentials out of the repo and out of checked-in `.env` files.

## Local Development

Use a file path in your local `.env`:

```bash
GOOGLE_APPLICATION_CREDENTIALS=/Users/joe/.config/memkit/google-service-account.json
```

That file should live outside the repo and should be readable only by the current user.

## Container / Hosted Deployments

For deployed environments, prefer storing the service-account JSON as a secret value and letting the runtime materialize it into a file.

The Docker image supports this directly:

- If `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` is present at container startup, the entrypoint writes it to a locked-down file.
- The entrypoint then exports `GOOGLE_APPLICATION_CREDENTIALS` to that file path before starting `mk`.
- The raw `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` variable is unset before the main process starts.

Default file path inside the container:

```bash
/run/secrets/memkit/google-service-account.json
```

If `GOOGLE_APPLICATION_CREDENTIALS` is already set in the container, the entrypoint writes the file there instead.

## GitHub Actions / CI Direction

This repo does not currently include a deploy workflow, so there is no checked-in pipeline to patch yet.

When a deployment workflow is added, the intended pattern is:

1. Store the Google service-account JSON as a GitHub Actions secret.
2. Pass that secret to the deployment target as `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON`.
3. Let the container entrypoint write the runtime file and set `GOOGLE_APPLICATION_CREDENTIALS`.

Avoid baking the credential file into the Docker image or committing it to the repo.

## AWS Direction

For AWS, the preferred shape is similar:

- Store the JSON in AWS Secrets Manager or SSM Parameter Store.
- Inject it into the task or container as `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON`.
- Let the container entrypoint materialize the file at runtime.

That keeps the image reusable and avoids distributing long-lived Google credentials in source control or build artifacts.
