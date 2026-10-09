# AUR packaging

Two AUR packages are prepared:

| Package | Directory | Source |
| --- | --- | --- |
| `omafanctrl` (stable) | [`omafanctrl/`](omafanctrl/) | The `v1.1.0` release tarball |
| `omafanctrl-git` | [`PKGBUILD`](PKGBUILD) | The `master` branch |

Both build the whole Cargo workspace and install the daemon, CLI, bar status
module, GUI, the D-Bus policy, polkit action, systemd unit, `ec_sys` drop-ins,
desktop entry, icon, the omarchy-shell bar module helper, and a default
`/etc/omafanctrl/TPFanControl.ini`.

## Building locally

```sh
cd packaging/aur/omafanctrl
makepkg -si
```

## Publishing the stable package

The AUR hosts one package per repository. To publish `omafanctrl`:

1. **Tag the release** in the main repository:

   ```sh
   git tag -a v1.1.0 -m "omafanctrl v1.1.0"
   git push origin v1.1.0
   ```

2. **Fill in the checksum.** The `PKGBUILD` ships with `sha256sums=('SKIP')`
   because the release tarball does not exist until the tag is pushed. Replace
   it with the real checksum:

   ```sh
   cd packaging/aur/omafanctrl
   updpkgsums
   ```

3. **Regenerate `.SRCINFO`** (the AUR rejects a stale one):

   ```sh
   makepkg --printsrcinfo > .SRCINFO
   ```

4. **Push to the AUR** (requires an AUR account and an SSH key registered with
   the AUR):

   ```sh
   git clone ssh://aur@aur.archlinux.org/omafanctrl.git aur-omafanctrl
   cp PKGBUILD .SRCINFO aur-omafanctrl/
   cd aur-omafanctrl
   git add PKGBUILD .SRCINFO
   git commit -m "omafanctrl 1.1.0"
   git push
   ```

## Publishing the `-git` package

The `-git` package derives its version from the repository, so it needs no
release tarball:

```sh
cd packaging/aur
makepkg --printsrcinfo > .SRCINFO
# then push PKGBUILD and .SRCINFO to ssh://aur@aur.archlinux.org/omafanctrl-git.git
```

## Verifying a package

```sh
namcap PKGBUILD
namcap omafanctrl-1.1.0-1-x86_64.pkg.tar.zst
```
