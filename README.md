# dotdeploy-rs

<p align="center">
<img src="https://github.com/FrauH0lle/dotdeploy-rs/assets/10484857/42731565-6950-4671-8edd-f73a10fb3c80" width="500">
</p>

## Description

dotdeploy is a dotfile manager that deploys configuration files, installs
packages, runs tasks and manages system state across multiple machines. Modules
organize related configuration and can depend on each other, while host-specific
configurations tie everything together for each machine.

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
# Removes the module corresponding to the hostname, if was previously
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
dotdeploy update module1 module2 [...]
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

The install/remove package commands can also be configured per-host via
`dotdeploy_config.toml` in the host directory:

``` toml
# Example: Fedora with dnf
install_pkg_cmd = [ "sudo", "dnf", "install", "-y" ]
remove_pkg_cmd = [ "sudo", "dnf", "remove", "-y" ]
```

## Directory Structure

A typical dotdeploy setup looks like this:

```
~/.dotfiles/
├── hosts/
│   ├── shared/           # Shared configuration included by multiple hosts
│   │   ├── env-sh.toml
│   │   ├── hosts.toml
│   │   └── ...
│   ├── myhostname/       # Host-specific configuration
│   │   ├── config.toml   # Main host config (dependencies, includes, files)
│   │   ├── dotdeploy_config.toml  # Host-specific dotdeploy settings
│   │   ├── home/         # Host-specific dotfiles
│   │   │   └── ##dot##config/
│   │   └── tweaks/       # System configuration files
│   └── workpc/
│       └── config.toml
├── modules/
│   ├── base/
│   │   └── config.toml
│   ├── shell/
│   │   ├── common/
│   │   │   ├── config.toml
│   │   │   ├── get_tldr.toml
│   │   │   └── get_eza.toml
│   │   ├── bash/
│   │   │   ├── config.toml
│   │   │   ├── bashrc
│   │   │   └── bash_profile
│   │   └── zsh/
│   │       └── config.toml
│   ├── editors/
│   │   └── emacs/
│   │       └── config.toml
│   └── dev/
│       ├── R/
│       │   ├── config.toml
│       │   ├── Makevars     # Template file
│       │   └── Rprofile
│       └── containers/
│           └── config.toml
└── scripts/              # Helper scripts
```

### The `##dot##` Convention

Files and directories in the source tree that start with `##dot##` have this
prefix replaced with `.` in the target path. This allows dotfiles to be stored
without the leading dot, making them visible in file managers and directory
listings.

For example, `hosts/myhost/home/##dot##config/` deployed with target `$HOME/*`
will become `$HOME/.config/`.

## Module Configuration

Each module is configured through one or more TOML files. The main configuration
file is typically `config.toml` within the module directory. A module
configuration can contain any combination of the following sections.

### Dependencies

Modules can depend on other modules. Dependencies are deployed automatically.

``` toml
depends_on = [ "shell/common" ]
```

A host configuration typically lists all modules it needs:

``` toml
depends_on = [
  "base",
  "shell/bash",
  "shell/zsh",
  "desktop/plasma",
  "editors/emacs",
  "dev/R",
  "dev/containers",
  "services/samba",
]
```

### Includes

Split module configuration across multiple files. Includes can be conditional.

``` toml
# Simple includes list
includes = [ "reflector.toml", "other.toml" ]

# Conditional includes
[[includes]]
files = [ "pipewire.toml", "flathub.toml" ]

[[includes]]
files = [ "$DOD_HOSTS_ROOT/shared/arch-system.toml" ]
if = "(eq DOD_DISTRIBUTION_NAME 'arch')"

# Include different configs based on distribution
[[includes]]
files = [ "ubuntu-wsl.toml" ]
if = "(eq DOD_DISTRIBUTION_NAME 'ubuntu')"

[[includes]]
files = [ "fedora-container.toml" ]
if = "(eq DOD_DISTRIBUTION_NAME 'fedora')"
```

### Files

Deploy files by linking, copying, or creating them. Glob patterns are supported
for deploying multiple files at once.

``` toml
# Link a file (default type)
[[files]]
source = "bashrc"
target = "$HOME/.bashrc"

# Copy a file
[[files]]
source = "pacman.conf"
target = "/etc/pacman.conf"
type = "copy"

# Copy with glob pattern (deploy all files in a directory)
[[files]]
source = "tweaks/*"
target = "/etc/*"
type = "copy"

# Deploy files to HOME with ##dot## replacement
[[files]]
source = "home/*"
target = "$HOME/*"

# Copy a file processed as a Handlebars template
[[files]]
source = "$DOD_HOSTS_ROOT/shared/hosts"
target = "/etc/hosts"
type = "copy"
template = true
permissions = "644"

# Template file with context variables
[[files]]
source = "Makevars"
target = "$HOME/.R/Makevars"
type = "copy"
template = true

# Conditional file deployment
[[files]]
source = "wsl-binfmt.conf"
target = "/etc/binfmt.d/WSLInterop.conf"
type = "copy"
if = "(eq DOD_DISTRIBUTION_NAME 'ubuntu')"

# Deploy files in the setup phase (before packages and main config)
[[files]]
source = "makepkg.conf"
target = "/etc/makepkg.conf"
phase = "setup"
type = "copy"
```

**File fields reference:**

| Field         | Required | Default    | Description                                       |
|---------------|----------|------------|---------------------------------------------------|
| `target`      | yes      |            | Destination path                                  |
| `source`      | no*      |            | Source path relative to module directory          |
| `content`     | no*      |            | Inline content (alternative to `source`)          |
| `type`        | no       | `"link"`   | `"link"`, `"copy"`, or `"create"`                 |
| `phase`       | no       | `"config"` | `"setup"` or `"config"`                           |
| `template`    | no       | `false`    | Process as Handlebars template (copy/create only) |
| `owner`       | no       |            | File owner                                        |
| `group`       | no       |            | File group                                        |
| `permissions` | no       |            | File permissions in octal (e.g., `"644"`)         |
| `if`          | no       |            | Condition expression                              |

\* A file must have either `source` or `content`, not both.

### Packages

Install system packages. The install and remove commands are configured globally
or per-host (see [Configuration](#configuration)).

``` toml
# Fedora packages
[[packages]]
install = [ "emacs", "ripgrep", "fd-find", "nodejs" ]
if = "(eq DOD_DISTRIBUTION_NAME 'fedora')"

# Arch Linux packages
[[packages]]
install = [ "bash", "bash-completion" ]
if = "(eq DOD_DISTRIBUTION_NAME 'arch')"

# Ubuntu packages
[[packages]]
install = [ "podman", "distrobox", "flatpak" ]
if = "(eq DOD_DISTRIBUTION_NAME 'ubuntu')"
```

**Package fields reference:**

| Field     | Required | Description                       |
|-----------|----------|-----------------------------------|
| `install` | yes      | Array of package names to install |
| `if`      | no       | Condition expression              |

### Tasks

Execute shell commands or scripts during different lifecycle phases. Tasks
support `setup`, `config`, `update`, and `remove` phases, and can run before
(`pre` hook) or after (`post` hook, default) other operations in that phase.

``` toml
# Simple task with lifecycle phases
[[tasks]]
description = "uv - Python package installer"
[[tasks.config]]
description = "Install uv if not present"
shell = '''
if [ ! -f "${XDG_BIN_HOME:-$HOME/.local/bin}/uv" ]; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
fi
'''
[[tasks.update]]
description = "Update uv"
shell = "uv self update"
[[tasks.remove]]
description = "Remove uv"
shell = '''
uv cache clean
rm -f "${XDG_BIN_HOME:-$HOME/.local/bin}/uv"
rm -f "${XDG_BIN_HOME:-$HOME/.local/bin}/uvx"
'''

# Task using exec instead of shell
[[tasks]]
description = "Emacs"
[[tasks.config]]
description = "Install config"
exec = "$DOD_DOTFILES_ROOT/scripts/ensure_repo.sh"
args = [ "git@github.com:FrauH0lle/emacs.d.git", "~/.emacs.d" ]
hook = "pre"

# Task with a condition on the task group
[[tasks]]
description = "dotdeploy completions"
if = "(is_executable 'dotdeploy')"
[[tasks.config]]
description = "Install Bash completions"
shell = '''
mkdir -p "$HOME/.local/share/bash-completion/completions"
dotdeploy completions -s bash > "$HOME/.local/share/bash-completion/completions"/dotdeploy.bash
'''

# Conditional task definitions within a group
[[tasks]]
description = "tealdear"
[[tasks.config]]
description = "Ensure tealdear bash completions are installed"
exec = "$DOD_DOTFILES_ROOT/scripts/download_github.sh"
args = [ "install", "tealdeer-rs/tealdeer", "completions_bash",
         "$HOME/.local/share/bash-completion/completions/tldr.bash" ]
if = "(contains 'shell/bash' DOD_MODULES)"
[[tasks.config]]
description = "Ensure tealdear zsh completions are installed"
exec = "$DOD_DOTFILES_ROOT/scripts/download_github.sh"
args = [ "install", "tealdeer-rs/tealdeer", "completions_zsh",
         "$HOME/.config/zsh/completions/_tldr" ]
if = "(contains 'shell/zsh' DOD_MODULES)"

# Multiple tasks with hooks controlling execution order
[[tasks]]
description = "Zsh setup and maintenance"
[[tasks.config]]
description = "Ensure .zshenv symlink exists"
shell = 'ln -svf "$HOME"/.zshenv "$HOME"/.config/zsh/.zshenv'
[[tasks.update]]
description = "Reset zgenom"
shell = '''
if [ -f "$HOME/.local/share/zgenom/zgenom.zsh" ]; then
  zsh -c "source $HOME/.local/share/zgenom/zgenom.zsh && zgenom reset"
fi
'''
hook = "post"
[[tasks.remove]]
shell = '''
rm -f "$HOME"/.zshenv
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}"/zsh/
rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}"/zgenom
'''

# Enable/disable systemd services
[[tasks]]
description = "snapper"
[[tasks.config]]
description = "Enable services"
shell = '''
for SERVICE in snapper-cleanup.timer snapper-timeline.timer; do
  if ! systemctl is-enabled --quiet "$SERVICE"; then
    sudo systemctl enable --now "$SERVICE"
  fi
done
'''
[[tasks.remove]]
description = "Disable services"
shell = '''
for SERVICE in snapper-cleanup.timer snapper-timeline.timer; do
  sudo systemctl disable --now "$SERVICE"
done
'''
hook = "pre"
```

**Task group fields:**

| Field         | Required | Description                                |
|---------------|----------|--------------------------------------------|
| `description` | no       | Human-readable description                 |
| `if`          | no       | Condition for the entire task group        |
| `setup`       | no       | Array of task definitions for setup phase  |
| `config`      | no       | Array of task definitions for config phase |
| `update`      | no       | Array of task definitions for update phase |
| `remove`      | no       | Array of task definitions for remove phase |

**Task definition fields (within each phase):**

| Field         | Required | Default  | Description                  |
|---------------|----------|----------|------------------------------|
| `description` | no       |          | Human-readable description   |
| `shell`       | no*      |          | Shell command to execute     |
| `exec`        | no*      |          | Executable to run            |
| `args`        | no       |          | Arguments for `exec`         |
| `expand_args` | no       | `true`   | Expand variables in args     |
| `sudo`        | no       | `false`  | Run with privilege elevation |
| `hook`        | no       | `"post"` | `"pre"` or `"post"`          |
| `if`          | no       |          | Condition expression         |

\* A task must have either `shell` or `exec`, not both.

**Task lifecycle notes:**
* `remove` tasks run when the module gets removed but also when:
  1. the task definition changes
  2. the task is removed from the module configuration

### Generators

Generators create files by assembling content from a source file with optional
shebang, prepended, and appended content. Useful for building shell
initialization files.

``` toml
# Generate ~/.env.sh with a pathmunge helper function
[[generators]]
target = "$HOME/.env.sh"
source = "env.sh"
shebang = "#!/bin/sh"
prepend = """
pathmunge() {
    case ":${PATH}:" in
        *:"$1":*)
            ;;
        *)
            if [ "$2" = "after" ] ; then
                PATH=$PATH:$1
            else
                PATH=$1:$PATH
            fi
    esac
}
"""
append = "unset -f pathmunge"

# Simple generators without prepend/append
[[generators]]
source = "fzf.sh"
target = "$HOME/.fzf.sh"
shebang = "#!/bin/sh"

[[generators]]
source = "aliases.sh"
target = "$HOME/.aliases.sh"
shebang = "#!/bin/sh"
```

**Generator fields reference:**

| Field               | Required | Default | Description                              |
|---------------------|----------|---------|------------------------------------------|
| `target`            | yes      |         | Path of the generated file               |
| `source`            | yes      |         | Source file relative to module directory |
| `shebang`           | no       |         | Shebang line (e.g., `"#!/bin/sh"`)       |
| `comment_start`     | no       | auto    | Comment character (auto-detected by ext) |
| `prepend`           | no       |         | Content prepended before source          |
| `append`            | no       |         | Content appended after source            |
| `skip_auto_content` | no       | `false` | Skip auto-generated warning header       |
| `owner`             | no       |         | File owner                               |
| `group`             | no       |         | File group                               |
| `permissions`       | no       |         | File permissions in octal                |
| `if`                | no       |         | Condition expression                     |

### Messages

Display informational messages to the user during deployment or removal.

``` toml
# Shown on deploy (default)
[[messages]]
message = """
Remember to install the emacs configuration via 'emacs-config deploy'.
"""

# Shown on remove
[[messages]]
message = "Samba has been removed."
on_command = "remove"

# Detailed help message
[[messages]]
message = """
Manage Samba authentication for named users:

  sudo smbpasswd -a some_user   # Add user
  sudo smbpasswd -e some_user   # Enable user
  sudo smbpasswd -d some_user   # Disable user
  sudo smbpasswd -x some_user   # Remove user
"""
```

**Message fields reference:**

| Field        | Required | Default    | Description                           |
|--------------|----------|------------|---------------------------------------|
| `message`    | yes      |            | The message text to display           |
| `on_command` | no       | `"deploy"` | `"deploy"`, `"remove"`, or `"update"` |
| `if`         | no       |            | Condition expression                  |

### Context Variables

Define custom variables for use in Handlebars templates.

``` toml
[context_vars]
ncpu = "16"
```

These variables can then be referenced in template files using `{{ncpu}}`.

## Condition Expressions

Many configuration fields support an `if` condition that controls whether the
entry is active. Conditions use S-expression syntax.

### Available Functions

| Function          | Description                          | Example                                                                      |
|-------------------|--------------------------------------|------------------------------------------------------------------------------|
| `eq`              | Equality check                       | `(eq DOD_DISTRIBUTION_NAME 'arch')`                                          |
| `ne`              | Not equal                            | `(ne DOD_HOSTNAME 'workpc')`                                                 |
| `and`             | Logical AND                          | `(and (eq DOD_DISTRIBUTION_NAME 'arch') (eq DOD_HOSTNAME 'myhost'))`         |
| `or`              | Logical OR                           | `(or (eq DOD_DISTRIBUTION_NAME 'arch') (eq DOD_DISTRIBUTION_NAME 'fedora'))` |
| `not`             | Logical NOT                          | `(not (eq DOD_HOSTNAME 'workpc'))`                                           |
| `contains`        | Check if value is in a list/variable | `(contains 'shell/bash' DOD_MODULES)`                                        |
| `is_executable`   | Check if command exists in PATH      | `(is_executable 'dotdeploy')`                                                |
| `command_success` | Check if shell command exits with 0  | `(command_success 'test -f /etc/debian_version')`                            |
| `command_output`  | Get output of a shell command        | `(command_output 'uname -m')`                                                |

### Built-in Variables

These variables are available in conditions and Handlebars templates:

| Variable                   | Description                                                 |
|----------------------------|-------------------------------------------------------------|
| `DOD_DOTFILES_ROOT`        | Root directory of dotfiles (e.g., `~/.dotfiles`)            |
| `DOD_MODULES_ROOT`         | Root directory of modules                                   |
| `DOD_HOSTS_ROOT`           | Root directory of host configurations                       |
| `DOD_HOSTNAME`             | System hostname                                             |
| `DOD_DISTRIBUTION`         | Full distribution string (e.g., `"arch"`, `"ubuntu:22.04"`) |
| `DOD_DISTRIBUTION_NAME`    | Distribution name only (e.g., `"arch"`, `"ubuntu"`)         |
| `DOD_DISTRIBUTION_VERSION` | Distribution version (e.g., `"22.04"`, empty if none)       |
| `DOD_USE_SUDO`             | Whether sudo is enabled (`"true"` / `"false"`)              |
| `DOD_DEPLOY_SYS_FILES`     | Whether system file deployment is enabled                   |
| `DOD_USER_STORE`           | Path to user store directory                                |
| `DOD_MODULES`              | Array of all deployed module names                          |
| `DOD_CURRENT_MODULE`       | Name of the module currently being processed                |

Environment variables like `$HOME`, `$USER`, `$XDG_BIN_HOME` etc. are also
expanded in paths.

## Deployment Phases

dotdeploy processes modules in a defined order:

1. **Setup phase** — Initial system preparation (e.g., system config files needed
   before package installation). Tasks with `hook = "pre"` run first, then setup
   files are deployed, then tasks with `hook = "post"`.
2. **Package installation** — System packages are installed.
3. **Config phase** — Main configuration deployment. Tasks with `hook = "pre"` run
   first, then config files are deployed, then tasks with `hook = "post"`.

## Host Configuration Example

A complete host configuration ties modules and shared configs together:

``` toml
## Module dependencies
depends_on = [
  "base",
  "shell/bash",
  "shell/zsh",
  "desktop/plasma",
  "editors/emacs",
  "dev/R",
  "dev/containers",
  "backup/snapper"
]

## Shared configuration includes
[[includes]]
files = [
  "$DOD_HOSTS_ROOT/shared/env-sh.toml",
  "$DOD_HOSTS_ROOT/shared/fstrim.toml",
  "$DOD_HOSTS_ROOT/shared/hosts.toml",
  "$DOD_HOSTS_ROOT/shared/generators.toml"
]

## Distribution-specific includes
[[includes]]
files = [ "$DOD_HOSTS_ROOT/shared/arch-system.toml" ]
if = "(eq DOD_DISTRIBUTION_NAME 'arch')"

## Host-specific packages
[[packages]]
install = [ "nextcloud-client", "keepassxc", "strawberry" ]
if = "(eq DOD_DISTRIBUTION_NAME 'arch')"

## Host-specific files
[[files]]
source = "tweaks/*"
target = "/etc/*"
type = "copy"

[[files]]
source = "home/*"
target = "$HOME/*"

## Template variables
[context_vars]
ncpu = "16"
```
