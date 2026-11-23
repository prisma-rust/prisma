# Prisma Discord bot

Converts Discord `/bug` reports into GitHub issues. Reference implementation of [`docs/integrations.md`](../../docs/integrations.md)
§5: uses only **slash command**, no Message Content intent needed.

```text
/bug summary: Crash on startup
     steps:   Windows 11 / RTX 4070
                            Black screen after cargo run, RUST_LOG=info log below...
```

→ creates issue:

```text
Title: [Discord] Crash on startup
Labels: source/discord, needs/triage
Body: > 🤖 Transferred from Discord by prisma-agent...
            > Reporter / channel

            **Summary**
            > Crash on startup

            **Details / Reproduction steps**
      > Windows 11 / RTX 4070
            > Black screen after cargo run...
```

## Prerequisites

1. **Discord Application**: <https://discord.com/developers/applications>
      - **Bot** → Reset Token → copy to `.env` `DISCORD_TOKEN`
      - **OAuth2 → URL Generator**: Scopes `bot` + `applications.commands`;
          Bot Permissions `Send Messages`, `Read Message History`
      - Invite to server with generated URL
2. **GitHub App**: create with [`tools/github-app/create_app.py`](../github-app/create_app.py),
      install scope **only `Excurs1ons/Prisma`**.
3. Node.js 20+.

## run

```bash
cd tools/discord-bot
npm install
cp .env.example .env
# Edit .env: DISCORD_TOKEN + GitHub App credentials
npm start
```

First launch will:

- Exchange App private key for installation token;
- Check installation can access `GITHUB_OWNER/GITHUB_REPO`;
- Warn if installation scope includes multiple repos (tighten in installation settings);
- Register `/bug`. With `DISCORD_GUILD_ID` set registers only to that server, immediate; otherwise global,
    up to ~1 hour.

## Deployment

Resident process, systemd or pm2 recommended:

```ini
# /etc/systemd/system/prisma-discord-bot.service
[Unit]
Description=Prisma Discord bot
After=network-online.target

[Service]
WorkingDirectory=/opt/prisma/tools/discord-bot
EnvironmentFile=/opt/prisma/tools/discord-bot/.env
ExecStart=/usr/bin/node index.js
Restart=on-failure
User=prisma

[Install]
WantedBy=multi-user.target
```

On Windows use `pm2 start index.js --name prisma-bot`, or put this dir in a
small bot-only container. Provide private key via mount / env var, do not bake into image.

## Security

- `.env` and `github-app.pem` **never in git**; root `.gitignore` already ignores `*.pem`, `.env`.
- GitHub App install scope only `Excurs1ons/Prisma`, permissions Issues + Metadata only.
- Discord token leaked once must Reset; old token immediately invalid.
- Report text treated as **untrusted data**: only quote, never execute commands / scripts in it, never post secrets.
- Bot has no Message Content intent, can only read slash command args.

## Troubleshooting

| Symptom | Check |
|---|---|
| `Missing Access` / command not appearing | Invite URL includes `applications.commands`; `DISCORD_GUILD_ID` correct |
| `Resource not accessible by integration` | App installed to Prisma; Issues permission write |
| `Bad credentials` | Discord token Reset; `.env` loaded correctly |
| Exits at startup `cannot access ...` | installation not installed to target repo, or `GITHUB_OWNER/REPO` wrong |
| Issue creation fails | Labels `source/discord` / `needs/triage` exist (`sh tools/ci/ensure_labels.sh`) |
