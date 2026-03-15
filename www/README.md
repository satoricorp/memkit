# WWW boilerplate

Config-first product website with auth, dashboard, checkout, and JSON-driven content.

## What to edit

Update copy and product data in `/config`:

- `config/site.json` (brand, nav, footer, social)
- `config/marketing.json` (hero, sections, features, FAQ)
- `config/products.json` (products + pricing)
- `config/dashboard.json` (dashboard metrics + modules)
- `config/auth.json` (auth labels + redirects)

## Local dev

```bash
bun dev
```

Open http://localhost:3000

## Environment variables

Core:

```bash
CONVEX_DEPLOYMENT=dev:your-deployment-name
NEXT_PUBLIC_CONVEX_URL=https://your-deployment.convex.cloud
NEXT_PUBLIC_APP_URL=http://localhost:3000
```

Auth (Convex): set in Convex dashboard or via `npx convex env set`:

```
SITE_URL
AUTH_GITHUB_ID
AUTH_GITHUB_SECRET
AUTH_GOOGLE_ID
AUTH_GOOGLE_SECRET
JWT_PRIVATE_KEY
JWKS
```

AI chat endpoint (optional, `app/api/chat`):

```bash
AI_API_KEY=your_api_key
AI_BASE_URL=https://api.openai.com/v1
AI_MODEL=gpt-4o-mini
```

Checkout + billing (Polar):

```bash
POLAR_ACCESS_TOKEN=
POLAR_SANDBOX_ACCESS_TOKEN=
POLAR_PRODUCT_ID=
POLAR_SANDBOX_PRODUCT_ID=
SUCCESS_URL=http://localhost:3000
```

Email (Resend):

```bash
RESEND_API_KEY=
RESEND_AUDIENCE_ID=
WELCOME_FROM="Brand <welcome@yourcompany.com>"
```

## Notes

- Dashboard content is static and config-driven. Replace with real data later.
- `app/settings` and `app/checkout` are wired but optional. Remove if not needed.
