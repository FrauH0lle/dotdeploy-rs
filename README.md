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
