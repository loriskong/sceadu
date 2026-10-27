# GitHub Accounts

Sceadu is developed under the `loriskong` GitHub account (`imloriskong@gmail.com`). The `valaxkong` account is reserved for the robot projects on the same machine. Both identities can coexist: the switch only affects the current shell and, inside a repository, its local Git config.

| Local key | GitHub user | Email | SSH key |
|---|---|---|---|
| `imloriskong` | `loriskong` | `imloriskong@gmail.com` | `~/.ssh/id_ed25519_imloriskong` |
| `valaxkong` | `valaxkong@163.com` | `~/.ssh/id_ed25519` |

The key paths and names can be overridden with `SCEADU_<ACCOUNT>_NAME`, `SCEADU_<ACCOUNT>_EMAIL`, and `SCEADU_<ACCOUNT>_KEY`.

## One-time SSH key setup for imloriskong

```sh
ssh-keygen -t ed25519 -C "imloriskong@gmail.com" -f "$HOME/.ssh/id_ed25519_imloriskong"
cat "$HOME/.ssh/id_ed25519_imloriskong.pub"
```

Add the public key to GitHub under **Settings -> SSH and GPG keys -> New SSH key**. Verify:

```sh
ssh -i "$HOME/.ssh/id_ed25519_imloriskong" -o IdentitiesOnly=yes -T git@github.com
```

The expected message is `Hi imloriskong! You've successfully authenticated...`.

If the existing `valaxkong` key is not the one registered on GitHub, point `SCEADU_VALAXKONG_KEY` at the correct key.

## Switch accounts

```sh
. ./scripts/github-account.sh
github_use imloriskong
github_status
```

`github_use` exports `GIT_AUTHOR_*`, `GIT_COMMITTER_*`, and `GIT_SSH_COMMAND`. Inside a repository it also writes `user.name` and `user.email` to `.git/config`. It never stores tokens or copies private keys.

To make the functions available in new shells, add this line to `~/.bashrc`:

```sh
. "$HOME/Sceadu/scripts/github-account.sh"
```

## Rebuild the repository under imloriskong

The first local history was created by the `valaxkong` identity. To avoid publishing those commits, archive the old history, reinitialize the repository, and create a fresh initial commit as `imloriskong`.

```sh
# 1. Archive the old history outside the new history but inside the ignored .design directory.
git bundle create .design/valaxkong-history.bundle --all
git bundle verify .design/valaxkong-history.bundle

# 2. Reinitialize.
rm -rf .git
git init -b main
. ./scripts/github-account.sh
github_use imloriskong

# 3. Create a single fresh commit.
git add .
git commit -m "chore: initialize Sceadu"

# 4. Add the empty GitHub repository and push.
git remote add origin git@github.com:loriskong/sceadu.git
git push -u origin main
```

Create the GitHub repository first, empty and without a README, license, or `.gitignore`. A private repository is recommended until the project is ready to publish.

## Verify attribution

```sh
git log --format='%h %an <%ae> %s'
git status
```

No commit should reference `valaxkong` after the rebuild. The old bundle stays local and is ignored by Git; delete it when it is no longer needed.
