# Design Document Sync

The Feishu document **Sceadu Design Document** is the canonical source for architecture and product intent. This repository is the source of truth for code. Every push to the default branch must append a code-update entry to the design document so readers can see what changed without opening the repository.

The sync is append-only: it adds a short entry with the commit, author, date, and changed files. It never rewrites architecture sections. Design changes are made manually in the Feishu document in the same change set.

## Current mode

CI sync is **not enabled** yet. The Feishu bot can read the document but does not have edit permission, and no repository secrets are configured. Until that changes, sync manually after a push:

```sh
SKIP_LARK_CONFIG=1 FEISHU_AS=user FEISHU_DOC_TOKEN=... python3 scripts/update-design-doc.py
```

The command uses the current CLI user identity. Confirm that it is valid with `lark-cli auth status`. The GitHub workflow is manual-dispatch only; the push trigger is commented out until bot edit access and secrets are in place.

## Trigger

- Workflow: `.github/workflows/design-doc.yml`
- Trigger: `push` to `main`, plus `workflow_dispatch` for a manual test.
- Concurrency: one sync job at a time to avoid interleaved writes.

If the default branch is not `main`, update the workflow filter.

## What gets appended

A single paragraph at the end of the document:

```text
Code update · <short sha> · <date> · <author>
<commit subject>
Files: <up to 12 changed files>
```

The document has a dedicated section, `19. Code Changes`, that holds these entries.

## Feishu app setup

CI uses a Feishu app (bot) identity. It must not use a personal user token: user tokens expire quickly and would require a human to re-authorize the runner.

1. Open the Feishu developer console for the app.
2. Add the scopes:
   - `docx:document`
   - `docx:document:write_only`
   - `docx:document:readonly` (recommended for verification)
3. Publish a new app version so the scopes take effect.
4. Add the app as an editor on the design document. In the document's Share dialog, search for the app name and grant edit access.
5. If the Share dialog cannot find the app, use the permission API with a one-time user authorization:

   ```sh
   lark-cli drive permission.members create \
     --type docx \
     --token "$FEISHU_DOC_TOKEN" \
     --data '{"member_type":"appid","member_id":"<app_id>","perm":"edit"}' \
     --as user
   ```

   This is a high-risk write. Confirm the target document and app ID before running it with `--yes`.

The app ID is the `cli_...` value used by the Feishu app. Feishu supports `appid` as a collaborator member type, so the app can be granted edit access directly.

## GitHub secrets

These are required only when enabling the CI push trigger. Until then, use the manual command above.

| Secret | Value |
|---|---|
| `FEISHU_APP_ID` | Feishu app ID, for example `cli_xxx` |
| `FEISHU_APP_SECRET` | Feishu app secret |
| `FEISHU_DOC_TOKEN` | Document token from the design-document URL |

The workflow installs `@larksuite/cli`, configures it with the app credentials, and appends the entry as the bot. The app secret is passed on stdin and is never written to the repository.

## Manual test

1. Add the secrets.
2. Run the **Update design doc** workflow with `workflow_dispatch`.
3. Open the document and confirm that a new entry appears at the end.

For a local test with an already configured CLI:

```sh
SKIP_LARK_CONFIG=1 \
FEISHU_DOC_TOKEN=... \
python3 scripts/update-design-doc.py
```

Local runs use the currently configured CLI identity. Use `lark-cli auth status` to confirm it is a bot or user with edit access.

## Failure behavior

The job fails if:

- a required secret is missing;
- the Feishu app cannot obtain a tenant token;
- the app lacks the docx scopes or edit permission;
- the append request returns `ok: false`;
- the append request returns `ok: true` but `data.result` is not `success` (Feishu can report permission failures this way).

Do not mark the job as successful when the document was not updated. A red workflow is the signal that the design document is drifting.

## Security notes

- Use a dedicated Feishu app for CI if possible; do not reuse a personal app with broad scopes.
- Grant edit access to the single design document, not to a whole folder.
- Rotate the app secret if it is exposed.
- Keep `FEISHU_APP_SECRET` and `FEISHU_DOC_TOKEN` in CI secrets only.
