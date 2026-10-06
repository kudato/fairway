# Configuring Fairway

Fairway reads core and plugin settings from TOML files. A file groups
parameters into tables with headings such as `[example]`.

The [guides for individual core features](index.md) and the
[plugin guides](../plugin-guide/index.md) describe the table names,
accepted parameters, and their default values.

## Fairway directory

Fairway looks for settings files in `~/.fairway`. You can change this
directory by setting the `FAIRWAY_HOME` environment variable. If the variable
is set but empty, Fairway cannot determine the directory and reports an error.

If there are no settings files, Fairway uses the defaults for core and
plugin settings.

```text
~/.fairway/
├── config.toml
├── config.auto.toml
└── conf.d/
    ├── 10-server.toml
    └── 90-local/
        └── example.toml
```

`config.toml` and `conf.d` are the [configuration files](#configuration-files).

## Configuration files

Fairway reads:

- `config.toml` in the Fairway directory;
- all `*.toml` files in its `conf.d` subdirectory, including nested
  directories;
- `config.auto.toml` in the Fairway directory.

You do not need to create the files in advance. If `config.auto.toml` is
missing, Fairway creates it when loading settings. Fairway uses this file
to save settings changed programmatically.

If no file contains a given table, the defaults defined for it are used.

Start with `config.toml`. The `conf.d` directory lets you split settings
across several files.

Fairway loads settings once at startup, before doing the requested work.
If you change the settings files while `fairway` is running, it continues
to use the previously loaded values.

When help (`--help`) or the version (`--version`) is requested, or the
arguments are invalid, Fairway exits before loading settings.

## File order

Fairway reads `config.toml` first, followed by the files from `conf.d`
in lexicographic order of their paths, and finally `config.auto.toml`.
Values from files read later override previously loaded values.
Settings in `config.auto.toml` therefore take precedence over those in
`config.toml` and `conf.d`.

Paths are compared part by part: first the top-level names inside `conf.d`,
then, if they are equal, the names at the next level. The comparison is
case-sensitive and treats digits in names as characters:

- `conf.d/A.toml` is applied before `conf.d/a.toml`;
- `conf.d/a/z.toml` is applied before `conf.d/b.toml`;
- `conf.d/10-base.toml` is applied before `conf.d/2-local.toml`.

To make the order obvious, start names with numbers of the same length:
`10-`, `50-`, `90-`.

## Merging values

Fairway merges tables with the same name. A later file changes only the
parameters it lists; the others stay. Nested tables follow the same rule,
while strings, numbers, and arrays are replaced as a whole.

`config.toml`:

```toml
[example]
listen = "127.0.0.1:8787"
idle_ttl = 300
```

`conf.d/10-server.toml`:

```toml
[example]
listen = "0.0.0.0:9000"
```

`conf.d/90-local/example.toml`:

```toml
[example]
idle_ttl = 60
```

The first additional file changes `listen`, and the second changes
`idle_ttl`. If `config.auto.toml` does not override these parameters, the
resulting table contains `listen = "0.0.0.0:9000"` and
`idle_ttl = 60`.

- Types and required parameters are checked after the files are merged,
  so each file can set only some of the parameters.
- Arrays, including arrays of tables `[[...]]`, are replaced as a whole:
  entries from different files are not combined.
- An empty `[example]` table in a later file does not reset the settings.
  To change a parameter, set a new value. To remove it from the
  configuration, delete it from every file where it appears.

## Which files are read

- In `conf.d`, Fairway reads files with the lowercase `.toml` extension.
  `README.md`, `10-server.toml.bak`, and `20-server.TOML` are skipped.
- Hidden files and directories are read too: `conf.d/.local.toml` is applied
  like any other file.
- Symbolic links to files and directories are followed, and `conf.d` itself
  can be a link. A broken link inside `conf.d` is an error, and so is a link
  to a parent directory that makes the walk loop.
- `config.toml` must be a regular file or a link to one.

## Values

- Strings are passed to the core and plugins as they are: `~` and environment
  variables are not expanded, and relative paths are not resolved against
  the file's location.
- Top-level tables must be declared by the core or plugins included in
  the program. A typo in the name, such as `[exampel]` instead of `[example]`,
  produces a loading error identifying the name and file.

## Errors

Before doing the requested work, Fairway checks all settings of the core
and included plugins.

If any table fails to load, Fairway prints an error and exits with code 1.
Default values are not substituted for invalid ones.
