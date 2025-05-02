# dotdeploy-rs

<p align="center">
<img src="https://github.com/FrauH0lle/dotdeploy-rs/assets/10484857/42731565-6950-4671-8edd-f73a10fb3c80" width="500">
</p>

## Description

## Usage 

### `deploy`

Deploy one or more modules to the system. This includes copying or linking files
to their destinations, creating backups of already present target files,
executing defined tasks, installing packages and displaying module info
messages.

``` sh
# Deploys the module corresponding to the hostname, if it exists.
dotdeploy deploy --host

# Deploys the specified modules
dotdeploy deploy module1 module2 [...]
```

### `sync`

Synchronize one or more modules' components. These can be a combination of
`files`, `tasks`, `packages` or `all` of the former.

``` sh
# Synchronizes any combination of files, tasks and/or packages for all 
# installed modules
dotdeploy sync [file tasks packages]

# Sync everything for all installed modules
dotdeploy sync all

# Sync everything for the hostname module
dotdeploy sync all --host

# Sync only specified modules
dotdeploy sync [file tasks packages] -- module1 module2 [...]
```

Note, that 
* `dotdeploy sync all` is equivalent to `dotdeploy deploy`
* `dotdeploy sync` only displays module messages when `all` is used

### `remove`

Remove one or more modules from the system. Files will be removed, backups
restored, tasks defined for the `remove` phase will be executed and info
messages will be displayed. `remove` will remove module dependencies if they are
not needed by another module.

``` sh
# Removes the module corresponding to the hostname, if it was previously 
# deployed.
dotdeploy remove --host

# Removes the specified modules
dotdeploy remove module1 module2 [...]
```

### `update`

Execute module maintenance tasks like pulling git repositories or downloading
the latest binary for a program.

``` sh
# Execute update tasks for all installed modules.
dotdeploy update

# Execute update tasks only for the specified modules
dotdeploy deploy module1 module2 [...]
```

### `lookup`

If a target file has been deployed by dotdeploy, `lookup` will return the source
file path. Otherwise it will just return the input file path.

``` sh
# Lookup file
dotdeploy lookup /myfile.txt
# => "/home/user/.dotfiles/modules/module1/myfile.txt" 

# You can combine this with your favorite editor command to edit directly the 
# correct file:
nano "$(dotdeploy lookup /myfile.txt | tr -d '"')"
# dotdeploy will return the filename in double quotes which usually need to be 
# removed.
```

### `uninstall`

Completely remove dotdeploy from the system and restore the previous state. 

``` sh
# Remove all modules and cleanup
dotdeploy uninstall

# Remove all modules and cleanup without asking for confirmation
dotdeploy uninstall --force --no-ask
```

### `completions`

Generate shell completions. Supported shells are: 
* bash
* zsh
* fish
* elvish
* powershell

``` sh
# If you use bash-completion
mkdir ~/.local/share/bash-completion/completions
dotdeploy completions --shell bash > ~/.local/share/bash-completion/completions/dotdeploy.bash

# Generate completions for bash and zsh and write them into the specified 
# directory
dotdeploy completions --shell bash --shell zsh --out ~/my_completions_dir
```

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
  
