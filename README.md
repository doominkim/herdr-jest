<div align="center">
  <h1>herdr-jest</h1>
  <p><b>A Jest test explorer for herdr.</b><br>
  The core of the VS Code Jest extension in a herdr popup: browse the test tree, run what you pick, and read failures.</p>

  <p>
    <img src="https://img.shields.io/badge/version-0.1.0-blue?style=flat-square" alt="Version 0.1.0">
    <img src="https://img.shields.io/badge/status-in%20development-orange?style=flat-square" alt="In development">
    <img src="https://img.shields.io/badge/macOS%20%7C%20Linux-lightgrey?style=flat-square" alt="macOS and Linux">
  </p>
</div>

> **In development.** Expect rough edges and breaking changes before 1.0.

## See every test at a glance.

Open the popup and your spec files appear as file › describe › test trees, with ✓ ✗ ○ from the last run. `it.each` tables and `.only` / `.skip` variants are recognized.

## Run exactly what you need.

Run a file, a describe block, a single test, everything, or only what failed. Runs continue in a per-repository background process after you close the popup, and the results are still there when you open it again, even after a restart. If a run you left behind finds new failures, herdr shows a notification.

## Read failures in place.

Expected and received values are highlighted, and framework stack frames are hidden.

## Install

Requires herdr 0.7.4 or later, a Rust toolchain (`cargo`) for the build step, and a project that uses Jest.

```sh
herdr plugin install doominkim/herdr-jest
```

Bind a key in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "f2"
type = "shell"
command = "herdr plugin pane open --plugin herdr-jest --entrypoint popup --focus"
description = "Open herdr-jest"
```

## Keys

| Key | Action |
|---|---|
| `j` `k` | Move |
| `l` `h` | Expand / collapse |
| `Enter` | Run the selected file, describe, or test |
| `a` | Run all tests |
| `f` | Rerun failed tests |
| `F` | Show failed tests only |
| `/` | Search by name |
| `Ctrl+d` `Ctrl+u` | Scroll the detail pane |
| `X` | Stop the background process |
| `q` `Esc` `F2` | Close |

## Configure the Jest command

herdr-jest runs `npx jest` by default. To change it per repository, edit `config.toml` in `herdr plugin config-dir herdr-jest`:

```toml
[[repo]]
path = "~/Workspace/my-app/*"
command = "npx cross-env TZ=Asia/Seoul jest"
```

## License

MIT
