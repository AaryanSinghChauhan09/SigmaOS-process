//! SigmaOS-process: process table with lifecycle states, parent/child
//! tracking, nice values, and zombie reaping — the in-kernel process
//! management core.
//!
//! Replaces the earlier placeholder with a real implementation:
//! states follow the Linux task-state model, pids are allocated
//! monotonically, children of exited parents are reparented to init (pid 1).

use std::collections::BTreeMap;
use std::fmt;

/// Task states (mirrors `TASK_*` in the kernel scheduler).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessState {
    Ready,      // runnable, waiting for a CPU
    Running,    // on-CPU
    Blocked,    // sleeping on I/O or event
    Stopped,    // job-control stop (SIGSTOP/SIGTSTP)
    Zombie,     // exited, awaiting wait()/reap
    Dead,       // reaped
}

impl ProcessState {
    pub fn is_alive(self) -> bool {
        !matches!(self, Self::Zombie | Self::Dead)
    }
    pub fn is_runnable(self) -> bool {
        matches!(self, Self::Ready | Self::Running)
    }
}

/// Scheduling policy classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchedPolicy {
    Other = 0,   // SCHED_OTHER / CFS
    Fifo = 1,    // SCHED_FIFO
    RoundRobin = 2, // SCHED_RR
    Batch = 3,
    Idle = 5,
    Deadline = 6,
}

/// A process descriptor (task_struct analog).
#[derive(Debug, Clone)]
pub struct ProcessDescriptor {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub sid: u32,
    pub state: ProcessState,
    pub priority: i32,   // static priority / nice-derived
    pub nice: i32,       // -20..19
    pub policy: SchedPolicy,
    pub start_time_ms: u64,
    pub utime_ms: u64,
    pub stime_ms: u64,
    pub children: Vec<u32>,
}

impl ProcessDescriptor {
    pub fn total_cpu_ms(&self) -> u64 { self.utime_ms + self.stime_ms }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcError {
    NotFound(u32),
    DuplicatePid(u32),
    IllegalTransition { pid: u32, from: ProcessState, to: ProcessState },
    InitCannotExit,
    NiceOutOfRange(i32),
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "no such process {}", p),
            Self::DuplicatePid(p) => write!(f, "pid {} already exists", p),
            Self::IllegalTransition { pid, from, to } =>
                write!(f, "pid {}: illegal state transition {:?} -> {:?}", pid, from, to),
            Self::InitCannotExit => write!(f, "pid 1 cannot exit"),
            Self::NiceOutOfRange(n) => write!(f, "nice value {} out of range [-20, 19]", n),
        }
    }
}

impl std::error::Error for ProcError {}

/// The system process table.
#[derive(Debug)]
pub struct ProcessTable {
    procs: BTreeMap<u32, ProcessDescriptor>,
    next_pid: u32,
    now_ms: u64,
}

impl Default for ProcessTable {
    fn default() -> Self {
        let mut t = Self { procs: BTreeMap::new(), next_pid: 2, now_ms: 0 };
        // pid 1: init — always present, cannot exit
        t.procs.insert(1, ProcessDescriptor {
            pid: 1, ppid: 0, pgid: 1, sid: 1,
            state: ProcessState::Running,
            priority: 120, nice: 0, policy: SchedPolicy::Other,
            start_time_ms: 0, utime_ms: 0, stime_ms: 0, children: vec![],
        });
        t
    }
}

impl ProcessTable {
    pub fn new() -> Self { Self::default() }

    pub fn len(&self) -> usize { self.procs.len() }
    pub fn is_empty(&self) -> bool { self.procs.is_empty() }
    pub fn get(&self, pid: u32) -> Option<&ProcessDescriptor> { self.procs.get(&pid) }
    pub fn get_mut(&mut self, pid: u32) -> Option<&mut ProcessDescriptor> { self.procs.get_mut(&pid) }

    pub fn advance_time(&mut self, ms: u64) { self.now_ms += ms; }
    pub fn now(&self) -> u64 { self.now_ms }

    /// Fork: create a child of `parent_pid`. Fails if parent missing.
    pub fn spawn(&mut self, parent_pid: u32, policy: SchedPolicy, nice: i32)
        -> Result<u32, ProcError>
    {
        if !(-20..=19).contains(&nice) {
            return Err(ProcError::NiceOutOfRange(nice));
        }
        let (pgid, sid) = {
            let parent = self.procs.get(&parent_pid).ok_or(ProcError::NotFound(parent_pid))?;
            if !parent.state.is_alive() {
                return Err(ProcError::IllegalTransition {
                    pid: parent_pid, from: parent.state, to: ProcessState::Running,
                });
            }
            (parent.pgid, parent.sid)
        };
        let pid = self.alloc_pid();
        let child = ProcessDescriptor {
            pid,
            ppid: parent_pid,
            pgid,
            sid,
            state: ProcessState::Ready,
            priority: Self::priority_from_nice(nice),
            nice,
            policy,
            start_time_ms: self.now_ms,
            utime_ms: 0, stime_ms: 0,
            children: vec![],
        };
        self.procs.get_mut(&parent_pid).unwrap().children.push(pid);
        self.procs.insert(pid, child);
        Ok(pid)
    }

    /// Convenience: spawn a child of init.
    pub fn spawn_root(&mut self, policy: SchedPolicy, nice: i32) -> Result<u32, ProcError> {
        self.spawn(1, policy, nice)
    }

    fn alloc_pid(&mut self) -> u32 {
        loop {
            let pid = self.next_pid;
            self.next_pid += 1;
            if !self.procs.contains_key(&pid) { return pid; }
        }
    }

    /// nice -20..19 → static priority 1..139 (higher nice = lower priority).
    pub fn priority_from_nice(nice: i32) -> i32 {
        120 - nice
    }

    /// setpriority(2): adjust nice within range, clamped.
    pub fn set_nice(&mut self, pid: u32, nice: i32) -> Result<(), ProcError> {
        if !(-20..=19).contains(&nice) {
            return Err(ProcError::NiceOutOfRange(nice));
        }
        let p = self.procs.get_mut(&pid).ok_or(ProcError::NotFound(pid))?;
        p.nice = nice;
        p.priority = Self::priority_from_nice(nice);
        Ok(())
    }

    /// State transition with legality check.
    pub fn transition(&mut self, pid: u32, to: ProcessState) -> Result<(), ProcError> {
        let p = self.procs.get(&pid).ok_or(ProcError::NotFound(pid))?;
        let from = p.state;
        let legal = matches!(
            (from, to),
            (ProcessState::Ready, ProcessState::Running)
                | (ProcessState::Running, ProcessState::Ready)
                | (ProcessState::Running, ProcessState::Blocked)
                | (ProcessState::Blocked, ProcessState::Ready)
                | (ProcessState::Running, ProcessState::Stopped)
                | (ProcessState::Ready, ProcessState::Stopped)
                | (ProcessState::Stopped, ProcessState::Ready)
                | (ProcessState::Running, ProcessState::Zombie)
                | (ProcessState::Blocked, ProcessState::Zombie)
                | (ProcessState::Stopped, ProcessState::Zombie)
                | (ProcessState::Zombie, ProcessState::Dead)
        );
        if !legal {
            return Err(ProcError::IllegalTransition { pid, from, to });
        }
        if pid == 1 && to == ProcessState::Zombie {
            return Err(ProcError::InitCannotExit);
        }
        self.procs.get_mut(&pid).unwrap().state = to;
        if to == ProcessState::Zombie {
            self.reparent_children(pid);
        }
        Ok(())
    }

    /// Children of an exited process move to init (pid 1).
    fn reparent_children(&mut self, dead_pid: u32) {
        let children: Vec<u32> = self.procs.get(&dead_pid)
            .map(|p| p.children.clone()).unwrap_or_default();
        if children.is_empty() { return; }
        for &c in &children {
            if let Some(child) = self.procs.get_mut(&c) {
                child.ppid = 1;
            }
        }
        if let Some(init) = self.procs.get_mut(&1) {
            init.children.extend(children);
        }
    }

    /// wait4()-style reap: collect zombie children of `parent_pid`,
    /// mark them Dead, return their pids.
    pub fn reap(&mut self, parent_pid: u32) -> Vec<u32> {
        let children: Vec<u32> = self.procs.get(&parent_pid)
            .map(|p| p.children.clone()).unwrap_or_default();
        let mut reaped = Vec::new();
        for c in children {
            if self.procs.get(&c).map(|p| p.state) == Some(ProcessState::Zombie) {
                self.procs.get_mut(&c).unwrap().state = ProcessState::Dead;
                reaped.push(c);
            }
        }
        reaped
    }

    /// All zombies in the table.
    pub fn zombies(&self) -> Vec<u32> {
        self.procs.values()
            .filter(|p| p.state == ProcessState::Zombie)
            .map(|p| p.pid).collect()
    }

    /// Runnable set for the scheduler (Ready + Running).
    pub fn runnable(&self) -> Vec<u32> {
        self.procs.values()
            .filter(|p| p.state.is_runnable())
            .map(|p| p.pid).collect()
    }

    /// Charge CPU time to a process.
    pub fn charge_cpu(&mut self, pid: u32, user_ms: u64, sys_ms: u64) {
        if let Some(p) = self.procs.get_mut(&pid) {
            p.utime_ms += user_ms;
            p.stime_ms += sys_ms;
        }
    }
}

// Back-compat shim: the old placeholder API.
pub type ProcessTableLegacy = ProcessTable;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_present_and_cannot_exit_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        // Act & Assert
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(1).unwrap().ppid, 0);
        assert_eq!(
            t.transition(1, ProcessState::Zombie).unwrap_err(),
            ProcError::InitCannotExit
        );
    }

    #[test]
    fn test_spawn_tree_and_ppid_links_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        // Act
        let shell = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        let child = t.spawn(shell, SchedPolicy::Other, 5).unwrap();
        let grandchild = t.spawn(child, SchedPolicy::Other, 0).unwrap();
        // Assert
        assert_eq!(t.get(child).unwrap().ppid, shell);
        assert_eq!(t.get(child).unwrap().pgid, 1);
        assert_eq!(t.get(grandchild).unwrap().ppid, child);
        assert_eq!(t.get(shell).unwrap().children, vec![child]);
    }

    #[test]
    fn test_nice_priority_mapping_aaa() {
        // Arrange & Act & Assert
        assert_eq!(ProcessTable::priority_from_nice(0), 120);
        assert_eq!(ProcessTable::priority_from_nice(19), 101);
        assert_eq!(ProcessTable::priority_from_nice(-20), 140);
        let mut t = ProcessTable::new();
        let p = t.spawn_root(SchedPolicy::Other, -5).unwrap();
        assert_eq!(t.get(p).unwrap().nice, -5);
        assert_eq!(t.get(p).unwrap().priority, 125);
        assert!(matches!(t.spawn_root(SchedPolicy::Other, 20), Err(ProcError::NiceOutOfRange(20))));
    }

    #[test]
    fn test_exit_reparents_children_to_init_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        let a = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        let b = t.spawn(a, SchedPolicy::Other, 0).unwrap();
        let c = t.spawn(b, SchedPolicy::Other, 0).unwrap(); // child of b
        // Act: b runs then exits -> zombie; its child c reparents to init
        t.transition(b, ProcessState::Running).unwrap();
        t.transition(b, ProcessState::Zombie).unwrap();
        // Assert
        assert_eq!(t.get(c).unwrap().ppid, 1);
        assert!(t.get(1).unwrap().children.contains(&c));
        let _ = a;
    }

    #[test]
    fn test_reap_collects_only_zombies_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        let p1 = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        let p2 = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        let p3 = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        t.transition(p1, ProcessState::Running).unwrap();
        t.transition(p1, ProcessState::Zombie).unwrap();
        t.transition(p3, ProcessState::Running).unwrap();
        t.transition(p3, ProcessState::Zombie).unwrap();
        // Act
        let reaped = t.reap(1);
        // Assert: only zombies; p2 still running
        assert_eq!(reaped.len(), 2);
        assert!(reaped.contains(&p1) && reaped.contains(&p3));
        assert_eq!(t.get(p1).unwrap().state, ProcessState::Dead);
        assert_eq!(t.get(p2).unwrap().state, ProcessState::Ready);
    }

    #[test]
    fn test_illegal_transition_rejected_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        let p = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        // Act & Assert: Ready -> Zombie illegal (must run first)
        let err = t.transition(p, ProcessState::Zombie).unwrap_err();
        assert!(matches!(err, ProcError::IllegalTransition { .. }));
        t.transition(p, ProcessState::Running).unwrap();
        t.transition(p, ProcessState::Zombie).unwrap();
        // Zombie -> Running illegal
        assert!(t.transition(p, ProcessState::Running).is_err());
        // Zombie -> Dead legal
        t.transition(p, ProcessState::Dead).unwrap();
    }

    #[test]
    fn test_spawn_from_dead_parent_fails_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        let p = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        t.transition(p, ProcessState::Running).unwrap();
        t.transition(p, ProcessState::Zombie).unwrap();
        // Act & Assert
        assert!(matches!(t.spawn(p, SchedPolicy::Other, 0),
            Err(ProcError::IllegalTransition { .. })));
        assert!(matches!(t.spawn(9999, SchedPolicy::Other, 0),
            Err(ProcError::NotFound(9999))));
    }

    #[test]
    fn test_runnable_and_zombies_filters_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        let a = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        let b = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        t.transition(a, ProcessState::Blocked).unwrap_err(); // Ready->Blocked illegal
        t.transition(a, ProcessState::Running).unwrap();
        t.transition(a, ProcessState::Blocked).unwrap();
        t.transition(b, ProcessState::Running).unwrap();
        t.transition(b, ProcessState::Zombie).unwrap();
        // Act & Assert
        let runnable = t.runnable();
        assert!(runnable.contains(&1));
        assert!(!runnable.contains(&a)); // blocked
        assert!(!runnable.contains(&b)); // zombie
        assert_eq!(t.zombies(), vec![b]);
    }

    #[test]
    fn test_cpu_accounting_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        let p = t.spawn_root(SchedPolicy::Other, 0).unwrap();
        // Act
        t.charge_cpu(p, 30, 10);
        // Assert
        assert_eq!(t.get(p).unwrap().total_cpu_ms(), 40);
    }

    #[test]
    fn test_pid_allocation_monotonic_no_reuse_aaa() {
        // Arrange
        let mut t = ProcessTable::new();
        // Act
        let pids: Vec<u32> = (0..5).map(|_| t.spawn_root(SchedPolicy::Other, 0).unwrap()).collect();
        // Assert
        assert_eq!(pids, vec![2, 3, 4, 5, 6]);
    }
}
