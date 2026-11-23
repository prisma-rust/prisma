# GitHub App setup (manifest flow)

GitHub **has no** REST API to "create an App". The only automated path is official
[App Manifest flow](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest):
confirm once in browser, GitHub callbacks a one-time code, script exchanges it for App ID, private key and client secret.

`create_app.py` does all steps except "click button":

1. Start a local callback server;
2. Generate manifest and open GitHub create page;
3. You click **Create GitHub App**;
4. Script exchanges credentials, writes outside repo, prints install link;
5. You click **Install**, script records `installation_id`.

2FA and account password always on your side, script never touches them.

## Usage

```bash
# Personal account, default App name prisma-agent, credentials written to ~/.prisma/github-app
python tools/github-app/create_app.py

# Organization account
python tools/github-app/create_app.py --owner my-org --owner-type org --name prisma-agent

# Only print manifest URL, do not start server
python tools/github-app/create_app.py --dry-run
```

Default callback address is `http://localhost:<random port>/callback`, no public network needed.
If not completed within 900 seconds script exits; created App and credentials remain, rerun creates a new App.

## create permissions

- Repository permissions: **Issues: Read and write**, **Metadata: Read-only**
- No webhook subscription (`default_events: []`)
- Private App, installed only to `Excurs1ons/Prisma`

On GitHub install page **only check Prisma**, do not select "All repositories".

## Credential location

Default outside repo:

```text
%USERPROFILE%\.prisma\github-app\
├── github-app.pem      # App private key (0600)
├── github-app.json     # app_id / slug / client secrets / installation_id
└── create-url.txt      # This manifest URL (can delete)
```

`github-app.json` contains `client_secret` / `webhook_secret`, also sensitive.
**Do not commit `.pem` or this JSON to git**; repo already ignores `*.pem`, `.secrets/`, `.env`.

## Env vars for bot

```bash
GITHUB_APP_ID=<app_id in github-app.json>
GITHUB_APP_PRIVATE_KEY_PATH=<absolute path to github-app.pem>
GITHUB_INSTALLATION_ID=<value recorded after install; if empty bot lists installations>
```

Bot exchanges App JWT for 1-hour installation token; do not call REST directly with private key.
If `installation_id` not recorded, bot can first `GET /app/installations` to find repo installation.

## Rotate / revoke

- Private key leaked: GitHub App settings → **Private keys** → delete old key → **Generate a private key**, update
    the new file `GITHUB_APP_PRIVATE_KEY_PATH` points to.
- No longer needed: App settings → **Delete GitHub App**; installation auto-invalidates.
- Rerunning script creates **new App**, does not overwrite old.

## Why cannot be fully automated

GitHub requires App creation and installation confirmed by logged-in user in browser, a security boundary
preventing third parties silently creating OAuth/App permissions. Script can automate manifest, callback, credential exchange and install ID.
