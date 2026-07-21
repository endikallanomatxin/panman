# Panman

Panman is a declarative, cross-platform development environment manager.

It composes `.panman/panman.toml` files from the current directory towards the filesystem root, imports immutable packages into content-addressed stores, builds lightweight environments and starts shells or commands inside them.

Panman is inspired by `nix develop`, but aims for a smaller configuration surface, first-class Windows support and interoperability with existing language ecosystems.

## MVP capabilities

- Hierarchical `.panman` discovery and composition.
- User store at `~/.panman/store`.
- Read-through system store at `/.panman/store` on Unix or `%PROGRAMDATA%/Panman/store` on Windows.
- Explicit `--system` writes; elevation alone never changes the target store.
- Local-directory packages and remote `.zip`, `.tar`, `.tar.gz` or `.tgz` archives.
- SHA-256 verification for every remote archive.
- Transactional package imports and immutable store paths.
- Generated environments with a dedicated `bin` directory.
- `panman sync`, `panman sh`, `panman run` and named scripts.
- Optional automatic startup from Bash or PowerShell.
- Linux, macOS and Windows CI.

The first MVP intentionally does not yet provide a package catalog or resolve names such as `node@22`. Its package/store/environment model is the foundation on which registries and ecosystem adapters will be added.

## Install for development

```bash
cargo install --path .
```

Or copy the current executable into Panman's stable bootstrap path:

```bash
panman self-install
```

## Create an environment

```bash
mkdir example
cd example
panman init
```

This creates:

```text
.panman/
└── panman.toml
```

Example manifest:

```toml
[environment]
name = "example"

[shell]
program = "zsh"

[env]
EDITOR = "nvim"

[scripts]
test = "cargo test"

[packages.internal-tool]
source = "path"
path = "./tools/internal-tool"
bins = ["bin/internal-tool"]

[packages.remote-tool]
source = "archive"
url = "https://example.com/remote-tool-x86_64-linux.tar.gz"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
strip_components = 1
bins = ["bin/remote-tool"]
```

Paths are resolved relative to the project directory containing `.panman`, not relative to the manifest file itself.

## Commands

### Inspect the effective environment

```bash
panman status
```

### Add a local package

```bash
panman add internal-tool \
  --path ./tools/internal-tool \
  --bin bin/internal-tool
```

### Add a verified archive

```bash
panman add remote-tool \
  --url https://example.com/remote-tool.tar.gz \
  --sha256 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
  --strip-components 1 \
  --bin bin/remote-tool
```

`panman add` currently rewrites the manifest using canonical TOML formatting. Comment-preserving edits are planned.

### Materialize the environment

```bash
panman sync
```

Panman first checks the system store, then the user store. Missing artifacts are imported into the user store.

To deliberately write missing artifacts to the system store:

```bash
sudo panman sync --system
```

### Open a shell

```bash
panman sh
```

### Run a command

```bash
panman run -- cargo test
```

### Run a named script

```bash
panman run test
```

### Enable automatic shell startup

```bash
panman shell enable
```

This copies the executable to `~/.panman/bin/panman` and installs an idempotent managed block in `.bashrc` on Unix or the PowerShell profile on Windows.

Disable it with:

```bash
panman shell disable
```

Set `PANMAN_DISABLE=1` to bypass automatic startup. Panman also sets `PANMAN_SHELL_ACTIVE=1` to prevent recursion.

## Hierarchical composition

Given:

```text
~/.panman/panman.toml
~/projects/.panman/panman.toml
~/projects/app/.panman/panman.toml
```

Starting Panman inside `~/projects/app` composes all three manifests. More specific manifests override parent values by key.

This enables personal tools, organization defaults, monorepo configuration and subproject-specific additions without duplicating configuration.

## Development

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features
cargo test --all-targets --all-features
```

See [`SPEC.md`](SPEC.md) for the product model and planned direction.
