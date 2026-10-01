# Configuring Fairway

Fairway reads plugin settings from TOML files. A file groups parameters
into tables with headings such as `[harness]`. The plugin's documentation
describes the table names, accepted parameters, and their default values.

The examples below use a hypothetical plugin with the `[harness]` table
and the `listen` and `idle_ttl` parameters.

## Fairway directory

Fairway looks for settings in `~/.fairway`, where `~` is the user's home
directory. The `FAIRWAY_HOME` environment variable sets a different
directory:

```sh
export FAIRWAY_HOME=/srv/fairway
```

If `FAIRWAY_HOME` is a relative path, the directory it selects depends on
where `fairway` is started. For example, `.fairway` selects
`/work/project/.fairway` when started from `/work/project`.
Use an absolute path to select the same directory on every run.

If the variable is set but empty, Fairway cannot determine the directory
and reports an error:

```text
error: could not initialize the Fairway filesystem: cannot resolve Fairway home: FAIRWAY_HOME is empty
```

You do not need to create this directory in advance. If there are no
settings files, Fairway uses the defaults provided by the plugins.
The directory itself is not created at startup. Example contents:

```text
~/.fairway/
├── config.toml
└── conf.d/
    ├── 10-server.toml
    └── 90-local/
        └── harness.toml
```

`config.toml` and `conf.d` are the [configuration files](#configuration-files).

## Configuration files

Fairway reads:

- `config.toml` in the Fairway directory;
- all `*.toml` files in its `conf.d` subdirectory, including nested
  directories.

Any file can contain tables of any plugins, and no file is required: if no
file contains a plugin's table, the plugin uses its default values. Fairway
itself never writes to these files.

Start with `config.toml`. The `conf.d` directory lets you split settings
across several files. For example, a deployment script can write shared
server settings to one file while you keep your changes in another.

Fairway loads settings once at startup, before the plugin starts its work.
If you change the settings files while `fairway` is running, it continues
to use the previously loaded values. Restart `fairway` to apply the new
settings.

When help (`--help`) or the version (`--version`) is requested, or the
arguments are invalid, Fairway exits before loading settings.

## File order

Fairway reads `config.toml` first, followed by the files from `conf.d`
in lexicographic order of their paths. Values from files read later
override previously loaded values.

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
[harness]
listen = "127.0.0.1:8787"
idle_ttl = 300
```

`conf.d/10-server.toml`:

```toml
[harness]
listen = "0.0.0.0:9000"
```

`conf.d/90-local/harness.toml`:

```toml
[harness]
idle_ttl = 60
```

The first additional file changes `listen`, and the second changes
`idle_ttl`. The plugin gets `listen = "0.0.0.0:9000"` and `idle_ttl = 60`.

- The plugin checks types and required parameters after the files are
  merged, so each file can set only some of the parameters.
- Arrays, including arrays of tables `[[...]]`, are replaced as a whole:
  entries from different files are not combined.
- An empty `[harness]` table in a later file does not reset the settings.
  To change a parameter, set a new value. To remove it from the
  configuration, delete it from every file where it appears. The plugin
  supplies a default only if it allows that parameter to be omitted.

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

- Strings are passed to the plugin as they are: `~` and environment
  variables are not expanded, and relative paths are not resolved against
  the file's location. The plugin's documentation describes how it treats
  relative paths.
- Fairway checks the syntax of the entire file. If a table does not belong
  to any available plugin, its parameters are skipped. A typo in the name,
  such as `[harnes]` instead of `[harness]`, therefore produces no error,
  but the settings in that table are not applied.

## Errors

Before doing the requested work, Fairway checks the settings of every
plugin included in the program. If any plugin's settings fail to load,
Fairway prints the error and exits with code 1. This also applies to
plugins you did not intend to use in this run. Default values never
replace invalid ones.

Loading fails if:

- any file has a TOML syntax error, even in a table that no plugin reads;
- a plugin's table is replaced by another value, such as `harness = 1`;
- parameters do not follow the plugin's rules: for example, a string is
  given instead of a number, a required parameter is missing, or the plugin
  rejects unknown parameters;
- a file or directory cannot be read, or a link in `conf.d` is broken or
  makes the walk loop.

For example, a plugin may reject an unknown `port` parameter:

```text
error: could not load Fairway configuration: invalid settings in namespace "harness", assembled from:
  /home/user/.fairway/config.toml
  /home/user/.fairway/conf.d/10-server.toml
  /home/user/.fairway/conf.d/90-local/harness.toml: unknown field `port`, expected `listen` or `idle_ttl`
```

In the message, `namespace "harness"` means the `[harness]` table.
The listed files each supply part of its settings, in the order they were
read. The cause is printed after the last file, but it applies to the
resulting table: the `port` parameter can be in any of the listed files.
