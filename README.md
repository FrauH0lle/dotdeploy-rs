# dotdeploy-rs

<p align="center">
<img src="https://github.com/FrauH0lle/dotdeploy-rs/assets/10484857/42731565-6950-4671-8edd-f73a10fb3c80" width="500">
</p>

## Description

### Commands

#### deploy
Handles complete deployment of modules from start to finish. This command:
- Checks and resolves module dependencies
- Processes configuration templates
- Deploys files in three stages (setup, config, update)
- Manages required software packages
- Detects file conflicts
- Stores important messages for later reference

#### update
Keeps deployed modules up-to-date. Use this to:
- Update specific modules or all modules at once
- Run maintenance tasks saved during deployment
- View stored messages about module updates
- Keep everything synchronized
- Safely refresh without changing existing configs (won't touch your existing configurations)

#### remove
Safely uninstalls modules and cleans up after them. This:
- Removes modules and their dependencies
- Deletes managed files while restoring backups
- Cleans up related software packages
- Updates remaining files to work without removed modules
- Prevents accidental removal of manually installed modules
- Clears old command records

#### lookup
Helps track down where deployed files came from. This:
- Checks both user-specific and system-wide records
- Shows original source paths for deployed files
- Handles files with unusual characters in their paths
- Works even if files weren't deployed through dotdeploy
- Great for debugging or checking file origins

## Configuration

dotdeploy's configuration file is by default located at
`~/.config/dotdeploy/config.toml`. Below are the configuration options together
with their defaults.

``` toml
# Basic options
# Show what would happen without making changes
dry_run = false

# Skip confirmations for destructive operations
force = false 

# Assume "yes" instead of prompting for confirmations
noconfirm = false

# Path configurations
# Root directory containing dotfiles
dotfiles_root = "~/.dotfiles"

# Directory containing module definitions
modules_root = "~/.dotfiles/modules"

# Directory containing the host modules 
hosts_root = "~/.dotfiles/hosts"

# Path for user-specific data storage
user_store_path = "~/.local/share/dotdeploy"

# Directory for storing log files
logs_dir = "~/.local/share/dotdeploy/logs"

# System detection (auto-detected if not specified)
# Override detected hostname
# Example: "myhost"
hostname = ""

# Override detected distribution (format: "id:version")
# Example: "ubuntu:22.04"
distribution = ""

# Privilege management
# Use sudo for privilege elevation
use_sudo = true

# Command to use for privilege elevation (sudo or doas)
sudo_cmd = "sudo"

# Allow deploying files outside user's HOME directory
deploy_sys_files = true

# Package management
# Command to install packages (with flags)
# Example: ["sudo", "dnf", "install", "-y"]
install_pkg_cmd = []

# Command to remove packages (with flags)
# Example: ["sudo", "dnf", "remove", "-y"]
remove_pkg_cmd = []

# Skip package installation during deployment
skip_pkg_install = false

# Logging
# Maximum number of log files to retain
logs_max = 15
```

## Module configuration

### Tasks

``` toml
[[tasks]]
description = "Install and maintain tealdear"
[[tasks.on_deploy]]
shell = """
if [ ! -f $XDG_BIN_HOME/tldr ]; then
  wget -O $XDG_BIN_HOME/tldr https://github.com/tealdeer-rs/tealdeer/releases/latest/download/tealdeer-linux-x86_64-musl
  chmod +x $XDG_BIN_HOME/tldr
fi
"""
[[tasks.on_update]]
shell = """
wget -O $XDG_BIN_HOME/tldr https://github.com/tealdeer-rs/tealdeer/releases/latest/download/tealdeer-linux-x86_64-musl
chmod +x $XDG_BIN_HOME/tldr
"""
[[tasks.on_remove]]
shell = """
rm  -f $XDG_BIN_HOME/tldr
"""
```
* `on_remove` will run when the module gets removed but also when 
  1. the task definition changes
  2. the task is removed from the module configuration
  
