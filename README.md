# dotdeploy-rs

<p align="center">
<img src="https://github.com/FrauH0lle/dotdeploy-rs/assets/10484857/42731565-6950-4671-8edd-f73a10fb3c80" width="500">
</p>

A robust dotfile manager with advanced features:
✅ Conditional execution using Handlebars templates  
✅ Modular configuration management  
✅ Dry-run simulations  
✅ Privilege escalation handling  
✅ Change tracking with SQLite backend  
✅ Backup/restore functionality  

## Key Features

- **Conditional Operations** - Execute actions based on template evaluations
- **Multi-type Modules** - Manage files, packages, messages and custom actions
- **Safe Execution** - Automatic sudo handling and privilege separation
- **State Tracking** - SQLite store records all deployed files and versions
- **Backup System** - Automatic backups before overwriting files
- **Cross-platform** - Works on Linux and Unix-like systems

## Installation

```bash
git clone https://github.com/FrauH0lle/dotdeploy-rs.git
cd dotdeploy-rs
cargo install --path .
```

## Basic Usage

1. Create a module directory structure:
```bash
mkdir -p ~/.config/dotdeploy/modules/my_module
```

2. Create a module configuration file (`module.toml`):
```toml
[meta]
name = "my_module"
description = "My personal dotfiles"
priority = 50

[files]
"nvim" = { source = "configs/nvim", target = "~/.config/nvim" }
"zshrc" = { source = "configs/zsh/.zshrc", target = "~/.zshrc" }

[actions.pre]
setup_dirs = { exec = "mkdir -p ~/.cache/{zsh,nvim}" }

[packages]
brew = ["neovim", "zsh-completions"]
apt = ["zsh", "fonts-powerline"]
```

3. Run the deployment:
```bash
dotdeploy run --module my_module
```

## Configuration

The tool uses a hierarchical configuration system with:
1. Default built-in values
2. System-wide config file (`/etc/dotdeploy/config.toml`)
3. User-specific config file (`~/.config/dotdeploy/config.toml`)
4. Command-line arguments

Example configuration:
```toml
[core]
dry_run = false
verbosity = 1
max_parallel = 4

[storage]
path = "~/.local/share/dotdeploy/store.db"
backup_dir = "~/.cache/dotdeploy/backups"

[sudo]
command = "sudo"
timeout = 300
```

## Contributing
Contributions are welcome! Please follow the project's [commit guidelines](./commit_messages.md) and [coding conventions](./conventions.org).
