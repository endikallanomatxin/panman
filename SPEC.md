# Panman MVP specification

## Purpose

Panman provides declarative and reproducible development environments on Linux, macOS and Windows without installing tools into normal system locations by default.

The product model is:

```text
versioned manifests
+ immutable shared stores
+ generated environments
+ managed shell execution
= reproducible cross-platform development environments
```

## Manifest discovery

Panman searches for `.panman/panman.toml` from the current directory through every parent directory. Discovered manifests are composed from least specific to most specific.

Child definitions override parent definitions by key. This applies independently to packages, environment variables, scripts, the environment name and the selected shell.

## Stores

Panman recognizes two stores:

- system store: `/.panman/store` on Unix and `%PROGRAMDATA%/Panman/store` on Windows;
- user store: `~/.panman/store`.

Resolution checks the system store before the user store. A normal `panman sync` writes missing artifacts only to the user store.

Writing to the system store requires the explicit `--system` option. Running Panman with elevated privileges never implicitly selects the system store.

Projects contain manifests and lockfiles, not package payloads.

## Package identity

The MVP supports two package sources:

- `path`: a local directory, identified by a deterministic SHA-256 hash of its content and relative paths;
- `archive`: a remote archive, identified by a mandatory SHA-256 digest.

Installed package directories use the form:

```text
<digest-prefix>-<logical-name>
```

Imports are built in a temporary directory and committed to the store by rename. Existing store entries are reused.

## Environments

Panman builds lightweight environments under:

```text
~/.panman/environments/<environment-hash>/
```

An environment contains:

- a `bin` directory exposing declared executables;
- `environment.json` with its identity and provenance.

On Unix, executables are exposed using symbolic links. On Windows, Panman generates command shims to avoid requiring symbolic-link privileges.

The environment hash includes package resolutions, environment variables and the selected shell.

## Lockfile

The nearest active `.panman` directory receives `panman.lock` after synchronization.

The lockfile records:

- schema version;
- target platform;
- environment hash;
- exact package digest;
- source identity;
- selected store path;
- exposed executables.

The MVP records one target per lockfile. A later schema will retain multiple platform resolutions simultaneously.

## Execution

`panman sh` synchronizes the environment and starts the configured shell with:

- the generated `bin` directory prepended to `PATH`;
- composed manifest variables;
- `PANMAN_ENV_ROOT` pointing to the generated environment;
- `PANMAN_SHELL_ACTIVE=1` to prevent startup recursion.

`panman run` applies the same environment to either a direct command or a named script.

## Bootstrap

`panman self-install` copies the current executable to the stable bootstrap location:

```text
~/.panman/bin/panman
```

`panman shell enable` installs the bootstrap and adds a managed activation block to Bash or PowerShell startup configuration. The operation is idempotent and reversible.

`PANMAN_DISABLE=1` is the recovery escape hatch.

## Security properties

The MVP provides:

- mandatory SHA-256 verification of remote archives;
- rejection of archive path traversal by the extraction libraries;
- content-addressed local package imports;
- transactional store writes;
- explicit system-store selection;
- no execution of arbitrary installation hooks;
- no package build scripts executed with administrator privileges.

Repository trust prompts, signed registries and stronger supply-chain verification are planned after the basic package model stabilizes.

## Deferred after the first vertical slice

The following are deliberately outside the initial implementation:

- resolving friendly requests such as `node@22`;
- Panman registry and provider selection;
- npm, uv and Cargo tool adapters;
- multi-platform resolutions in one lockfile;
- comment-preserving manifest edits;
- garbage collection and reference tracking;
- signed indexes and trust database;
- automatic environment changes after `cd` within an already-running shell.

These features extend the implemented store and environment model rather than replacing it.
