use alloc::collections::VecDeque;

use super::driver::EINVAL;
use crate::log;

pub type TaskFn = fn(usize, i32);

#[derive(Clone, Copy)]
struct TaskEntry {
    priority: i32,
    func: TaskFn,
    context: usize,
    pending: i32,
}

pub struct Task {
    priority: i32,
    func: Option<TaskFn>,
    context: usize,
    pending: i32,
    queued: bool,
    initialized: bool,
}

impl Task {
    pub const fn new() -> Self {
        Self {
            priority: 0,
            func: None,
            context: 0,
            pending: 0,
            queued: false,
            initialized: false,
        }
    }
}

pub struct TaskQueue {
    name: &'static str,
    entries: VecDeque<TaskEntry>,
}

pub fn task_init(task: &mut Task, priority: i32, func: TaskFn, context: usize) {
    task.priority = priority;
    task.func = Some(func);
    task.context = context;
    task.pending = 0;
    task.queued = false;
    task.initialized = true;
    log!(
        "freebsd_kpi: task initialized priority={} context={:#x}",
        priority,
        context
    );
}

pub fn taskqueue_create(name: &'static str) -> TaskQueue {
    log!("freebsd_kpi: taskqueue {} created", name);
    TaskQueue {
        name,
        entries: VecDeque::new(),
    }
}

pub fn taskqueue_free(mut queue: TaskQueue) {
    let pending = queue.entries.len();
    queue.entries.clear();
    log!(
        "freebsd_kpi: taskqueue {} freed pending_cleared={}",
        queue.name,
        pending
    );
}

pub fn taskqueue_enqueue(queue: &mut TaskQueue, task: &mut Task) -> i32 {
    let Some(func) = task.func else {
        return EINVAL;
    };
    if !task.initialized {
        return EINVAL;
    }

    task.pending = task.pending.saturating_add(1);
    if !task.queued {
        queue.entries.push_back(TaskEntry {
            priority: task.priority,
            func,
            context: task.context,
            pending: task.pending,
        });
        task.queued = true;
    }

    log!(
        "freebsd_kpi: task enqueued queue={} pending={} len={}",
        queue.name,
        task.pending,
        queue.entries.len()
    );
    0
}

pub fn taskqueue_drain(queue: &mut TaskQueue, task: &mut Task) {
    queue
        .entries
        .retain(|entry| entry.context != task.context || entry.priority != task.priority);
    task.pending = 0;
    task.queued = false;
    log!(
        "freebsd_kpi: taskqueue {} drained len={}",
        queue.name,
        queue.entries.len()
    );
}

pub fn taskqueue_run(queue: &mut TaskQueue) {
    log!(
        "freebsd_kpi: taskqueue {} run len={}",
        queue.name,
        queue.entries.len()
    );
    while let Some(entry) = queue.entries.pop_front() {
        (entry.func)(entry.context, entry.pending);
    }
}

pub fn taskqueue_len(queue: &TaskQueue) -> usize {
    queue.entries.len()
}
