use super::{Task, TaskId};
use alloc::task::Wake;
use alloc::{collections::BTreeMap, sync::Arc};
use conquer_once::spin::OnceCell;
use core::task::{Context, Poll, Waker};
use crossbeam_queue::ArrayQueue;
use crate::utils::sync::Mutex;
use x86_64::instructions::interrupts;

pub static EXECUTOR: OnceCell<Mutex<Executor>> = OnceCell::uninit();

pub fn init_executor() {
    EXECUTOR
        .try_init_once(|| Mutex::new(Executor::new()))
        .unwrap()
}

pub fn sleep(duration: f64) {
    if !duration.is_finite() || duration <= 0.0 {
        return;
    }

    let now_ns = crate::driver::time::monotonic_ns();
    let duration_ns_f = duration * 1_000_000_000.0;
    let truncated_ns = duration_ns_f as u64;
    let duration_ns = if truncated_ns == 0 {
        1
    } else if (truncated_ns as f64) < duration_ns_f {
        truncated_ns.saturating_add(1)
    } else {
        truncated_ns
    };
    let deadline_ns = now_ns.saturating_add(duration_ns);

    // The periodic PIT used to wake this HLT loop incidentally. Under the
    // one-shot clockevent policy (#68), publish the actual wake deadline so an
    // otherwise idle boot/kernel context cannot sleep forever.
    crate::driver::time::clockevent::arm_kernel_hlt_wake(deadline_ns);
    let _irq_guard = crate::utils::sync::IrqGuard::new();
    while crate::driver::time::monotonic_ns() < deadline_ns {
        halt();
    }
    crate::driver::time::clockevent::clear_kernel_hlt_wake();
}

pub fn halt() {
    let disabled = !interrupts::are_enabled();
    interrupts::enable_and_hlt();
    if disabled {
        interrupts::disable();
    }
}

pub struct Executor {
    tasks: BTreeMap<TaskId, Task>,
    task_queue: Arc<ArrayQueue<TaskId>>,
    waker_cache: BTreeMap<TaskId, Waker>,
}

unsafe impl Sync for Executor {}
unsafe impl Send for Executor {}

struct TaskWaker {
    task_id: TaskId,
    task_queue: Arc<ArrayQueue<TaskId>>,
}

impl TaskWaker {
    fn new(task_id: TaskId, task_queue: Arc<ArrayQueue<TaskId>>) -> Waker {
        Waker::from(Arc::new(TaskWaker {
            task_id,
            task_queue,
        }))
    }

    fn wake_task(&self) {
        self.task_queue.push(self.task_id).expect("task_queue full");
    }
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_task();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_task();
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Executor {
        Executor {
            tasks: BTreeMap::new(),
            task_queue: Arc::new(ArrayQueue::new(100)),
            waker_cache: BTreeMap::new(),
        }
    }

    pub fn spawn(&mut self, task: Task) {
        let task_id = task.id;
        if self.tasks.insert(task.id, task).is_some() {
            panic!("task with same ID already in tasks");
        }
        self.task_queue.push(task_id).expect("queue full");
    }

    fn run_ready_tasks(&mut self) {
        // destructure `self` to avoid borrow checker errors
        let Self {
            tasks,
            task_queue,
            waker_cache,
        } = self;

        while let Some(task_id) = task_queue.pop() {
            let task = match tasks.get_mut(&task_id) {
                Some(task) => task,
                None => continue, // task no longer exists
            };
            let waker = waker_cache
                .entry(task_id)
                .or_insert_with(|| TaskWaker::new(task_id, task_queue.clone()));
            let mut context = Context::from_waker(waker);
            match task.poll(&mut context) {
                Poll::Ready(()) => {
                    // task done -> remove it and its cached waker
                    tasks.remove(&task_id);
                    waker_cache.remove(&task_id);
                }
                Poll::Pending => {}
            }
        }
    }

    pub fn run(&mut self) -> ! {
        loop {
            self.run_ready_tasks();
            self.sleep_if_idle();
        }
    }

    fn sleep_if_idle(&self) {
        use x86_64::instructions::interrupts::{self, enable_and_hlt};

        interrupts::disable();
        if self.task_queue.is_empty() {
            enable_and_hlt();
        } else {
            interrupts::enable();
        }
    }
}
