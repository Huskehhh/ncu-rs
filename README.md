# ncu-rs

A stupid simple and fast cli for updating your package.json dependencies.

## Installation

```bash
$ cargo install ncu-rs
```

## Usage

By default, points to the current directory `package.json`, but you can optionally specify a path.

```bash
$ ncu-rs -h
USAGE:
    ncu-rs [OPTIONS] [path]

ARGS:
    <path>    Optional path to package.json

OPTIONS:
    -h, --help       Print help information
    -u, --update     Enables updating of dep versions in package.json
    -V, --version    Print version information
```

A dry run:

```bash
$ ncu-rs ~/Documents/mycoolproject/package.json
```

A real run (will make changes to your `package.json`!):

```bash
$ ncu-rs -u ~/Documents/mycoolproject/package.json
```