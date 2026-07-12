# Cloudflare Pages deployment

The production site is deployed as a Cloudflare Pages **Direct Upload** project by [the GitHub Actions workflow](.github/workflows/deploy-cloudflare-pages.yml). The workflow runs only for pushes to `master` and manual dispatches; pull requests do not deploy and cannot consume the deployment secrets.

## 1. Create the Pages project

Install Node.js 22 or newer, authenticate Wrangler with the Cloudflare account that will own the site, and create the project once:

```sh
npx wrangler@4 login
npx wrangler@4 pages project create roady-car --production-branch main
```

This creates a Direct Upload Pages project named `roady-car`, with `main` as the Pages production-branch label used by deployments. To use another name, replace `roady-car` here and set the repository variable described below to exactly the same value.

Alternatively, in the Cloudflare dashboard select **Workers & Pages → Create application → Pages → Upload assets**, choose a project name, and complete the initial Direct Upload setup. Do not connect Cloudflare's Git integration; GitHub Actions performs the builds and uploads.

## 2. Create an API token

In Cloudflare, open **My Profile → API Tokens → Create Token → Create Custom Token**. Configure:

- **Permissions:** `Account` / `Cloudflare Pages` / `Edit`
- **Account Resources:** include the account that owns the Pages project

Create and copy the token. The account ID is shown on the Cloudflare account overview and in the Pages project's dashboard.

## 3. Configure GitHub

Open the repository's **Settings → Secrets and variables → Actions** and add these repository secrets:

- `CLOUDFLARE_API_TOKEN`: the custom API token
- `CLOUDFLARE_ACCOUNT_ID`: the owning Cloudflare account ID

Optionally add the repository variable `CLOUDFLARE_PAGES_PROJECT` with the Pages project name. If it is absent or empty, the workflow uses `roady-car`.

The workflow's GitHub environment is named `production`. Environment protection rules can be added in repository settings if deployment approval is desired; the Cloudflare credentials are repository secrets as listed above.

## 4. Deploy

Push to `master`, or open **Actions → Deploy to Cloudflare Pages → Run workflow**. The workflow builds and validates `dist/`, uploads it as a GitHub Actions artifact, and then runs the production deployment command:

```sh
npx wrangler@4 pages deploy dist --project-name "$PROJECT_NAME" --branch main
```

Cloudflare assigns the project a `pages.dev` address. Consult the workflow log or Cloudflare dashboard for the actual address; the repository does not claim a fixed live URL.

## Local production build and upload

Install Rust stable, the WebAssembly target, and the workflow's locked Trunk version, then build and validate the same output locally:

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked trunk --version 0.21.14
trunk build --release --cargo-profile wasm-release
python tools/check_release.py
```

To upload that `dist/` directory manually, authenticate with `npx wrangler@4 login` and run:

```sh
npx wrangler@4 pages deploy dist --project-name roady-car --branch main
```

Use the configured project name instead of `roady-car` when applicable.

## Custom domain

In the Cloudflare dashboard, open **Workers & Pages → the project → Custom domains → Set up a custom domain**, enter the hostname, and follow the DNS prompts. For a domain already using Cloudflare DNS, Cloudflare can add the required record. For an external DNS zone, add the CNAME target shown by Cloudflare and complete any requested ownership verification.

## Base-path caveat

The current Trunk public URL is relative (`./`), which works for the Pages root and keeps emitted asset links relative when the site is served below a subpath. If hosting or routing changes, preserve that relative base or deliberately update the public URL and verify asset, worker, and browser-navigation paths before deployment.
