use std::sync::Arc;

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde_json::Value;

use crate::modules::actions::ModuleAction;
use crate::modules::messages::ModuleMessages;
use crate::modules::packages::ModulePackages;
use crate::modules::Module;
use crate::phases::file_operations::FileOperation;
use crate::store::Stores;

enum Phase {
    Setup(Stage),
    Deploy(Stage),
    Config(Stage),
    Remove(Stage),
}

struct Stage {
    pre: Vec<Task>,
    main: Vec<Task>,
    post: Vec<Task>,
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            pre: vec![],
            main: vec![],
            post: vec![],
        }
    }
}

enum Task {
    FileOperation(FileOperation),
    PackageOperation(ModulePackages),
    ExecuteAction(ModuleAction),
    DisplayMessage(ModuleMessages),
}

struct Executor {
    phases: Vec<Phase>,
    stores: Arc<Stores>,
    context: Value,
}

impl Executor {
    /// Creates a new Executor from a list of modules and stores.
    pub fn new(modules: Vec<Module>, stores: Arc<Stores>, context: Value) -> Result<Self> {
        let phases = Self::process_modules(modules, &stores, &context)?;
        Ok(Self {
            phases,
            stores,
            context,
        })
    }

    /// Processes modules and creates phases with their respective tasks.
    fn process_modules(
        modules: Vec<Module>,
        stores: &Stores,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Vec<Phase>> {
        let mut phases = vec![
            Phase::Setup(Stage::default()),
            Phase::Deploy(Stage::default()),
            Phase::Config(Stage::default()),
            Phase::Remove(Stage::default()),
        ];

        for module in modules {
            Self::assign_module_to_phases(&mut phases, module, stores, context, hb)?;
        }

        Ok(phases)
    }

    /// Assigns a module's components to the appropriate phases and stages.
    fn assign_module_to_phases(
        phases: &mut [Phase],
        mut module: Module,
        stores: &Stores,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        // Implement the logic to assign module components to phases and stages
        // This is similar to your current assign_module_config function
        // ...

        // Evaluate module configurations against the provided context
        module
            .config
            .eval_conditionals(&context, hb)
            .with_context(|| {
                format!(
                    "Failed to evaluate conditionals for module '{}'",
                    module.name
                )
            })?;

        let module_name = module.name;

        Ok(())
    }

    /// Executes all phases in order.
    pub fn execute(&self) -> Result<()> {
        for phase in &self.phases {
            self.execute_phase(phase)?;
        }
        Ok(())
    }

    /// Executes a single phase.
    fn execute_phase(&self, phase: &Phase) -> Result<()> {
        let phase_name = match phase {
            Phase::Setup(_) => "Setup",
            Phase::Deploy(_) => "Deploy",
            Phase::Config(_) => "Config",
            Phase::Remove(_) => "Remove",
        };
        info!("Starting {} phase", phase_name);

        let stage = match phase {
            Phase::Setup(s) | Phase::Deploy(s) | Phase::Config(s) | Phase::Remove(s) => s,
        };

        self.execute_stage_tasks(&stage.pre, "pre")?;
        self.execute_stage_tasks(&stage.main, "main")?;
        self.execute_stage_tasks(&stage.post, "post")?;

        info!("Finished {} phase", phase_name);
        Ok(())
    }

    /// Executes tasks for a specific stage.
    fn execute_stage_tasks(&self, tasks: &[Task], stage_name: &str) -> Result<()> {
        if !tasks.is_empty() {
            info!("Executing {} stage tasks", stage_name);
            for task in tasks {
                self.execute_task(task)
                    .with_context(|| format!("Failed to execute task in {} stage", stage_name))?;
            }
        }
        Ok(())
    }

    /// Executes a single task.
    fn execute_task(&self, task: &Task) -> Result<()> {
        match task {
            Task::FileOperation(op) => self.execute_file_operation(op),
            Task::PackageOperation(op) => self.execute_package_operation(op),
            Task::ExecuteAction(action) => self.execute_action(action),
            Task::DisplayMessage(message) => self.display_message(message),
        }
    }

    // Implement methods for executing specific types of tasks
    fn execute_file_operation(&self, op: &FileOperation) -> Result<()> {
        // Implement file operations (copy, link, create, remove)
        // This would interact with the stores for tracking and backup
        // ...
        Ok(())
    }

    fn execute_package_operation(&self, op: &PackageOperation) -> Result<()> {
        // Implement package operations (install, remove)
        // ...
        Ok(())
    }

    fn execute_action(&self, action: &Action) -> Result<()> {
        // Execute the action (run a command)
        // ...
        Ok(())
    }

    fn display_message(&self, message: &Message) -> Result<()> {
        // Display the message
        println!("{}", message.content);
        Ok(())
    }
}

struct Logger<T: LogWriter> {
    writer: T,
}

impl<T> Logger<T>
where
    T: LogWriter,
{
    fn log(&self, message: &str) {
        self.writer.write_log(message);
    }
}

trait LogWriter {
    fn write_log(&self, message: &str);
}

struct ConsoleWriter;

impl LogWriter for ConsoleWriter {
    fn write_log(&self, message: &str) {
        println!("{}", message);
    }
}

fn bla() {
    let l = Logger {
        writer: ConsoleWriter,
    };
    l.log("hi");
    l.writer.write_log("hi");
}
