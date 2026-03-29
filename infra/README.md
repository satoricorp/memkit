# Infra Layout

`infra/deploy` is an optional private git submodule that owns AWS deployment code for the managed Memkit cloud service.

- If the submodule and deploy secrets exist, the GitHub Actions deploy workflow will build the app image and call the deploy scripts inside `infra/deploy`.
- If the submodule or deploy secrets are missing, the deploy workflow skips cleanly.
- Self-hosters can point `infra/deploy` at their own private deploy repo and configure their own AWS environment.

The main app repo should not carry committed AWS CDK or deploy scripts outside this submodule.
