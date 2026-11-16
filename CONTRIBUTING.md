# Contributing

Thanks for helping. Falconer is maintained by [lesquel](https://github.com/lesquel).

## Issues first

- **Bug:** open an issue with the *Bug report* template: what you did, what you expected, what
  happened, versions (`falconer-hook version`, app version from Settings) and logs with secrets
  removed.
- **Feature:** open an issue with the *Feature request* template before writing code, so we agree
  on the behavior first.
- **Security problem:** do not open a public issue. Use GitHub's *Report a vulnerability*
  (Security tab) so it stays private until it is fixed.

## Pull requests

1. Fork, then branch from `main` (`fix/...`, `feat/...`).
2. Keep one change per PR, with its tests and docs.
3. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` before pushing. CI runs the same checks and must be green.
4. Use [Conventional Commits](https://www.conventionalcommits.org/) (`fix:`, `feat:`, `docs:`…).
5. Open the PR against `main` and fill in the template. The maintainer reviews and merges.

By contributing you agree that your contribution is licensed under this repository's license.

## Releases

Nothing reaches users until it has been tested on a real device:

1. Merged work waits on `main`.
2. The maintainer tags a **release candidate**, `vX.Y.Z-rc.N`. Tags with a `-` are published
   as GitHub **pre-releases**: the installer's default ("latest") and the Releases page's
   "Latest" badge ignore them.
3. Testers install it with `install.sh --version v1.2.0-rc.1` and report problems as issues.
4. When the candidate works, the same commit is tagged `vX.Y.Z`: that is the release everyone
   gets.
