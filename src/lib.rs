#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessState { Running, Blocked, Zombie }

#[derive(Debug, Clone)]
pub struct ProcessDescriptor {
    pub pid: u64,
    pub state: ProcessState,
}

#[derive(Debug, Default)]
pub struct ProcessTable {
    processes: Vec<ProcessDescriptor>,
}

impl ProcessTable {
    pub fn new() -> Self { Self::default() }
    pub fn spawn(&mut self, pid: u64) {
        self.processes.push(ProcessDescriptor { pid, state: ProcessState::Running });
    }
    pub fn find(&self, pid: u64) -> Option<&ProcessDescriptor> {
        self.processes.iter().find(|p| p.pid == pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_process_table_operations() {
        let mut table = ProcessTable::new();
        table.spawn(1);
        let p = table.find(1).unwrap();
        assert_eq!(p.state, ProcessState::Running);
    }
}
