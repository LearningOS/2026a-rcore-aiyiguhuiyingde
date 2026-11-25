# lab1 · 第三章：从批处理到多道程序（以 `ch5` 分支代码为准）

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch5`（孤儿历史，单根提交 `Initialize ch5`，不继承前序分支的提交） |
| 主题 | 任务控制块、内核/用户栈、`__switch` 控制流交接、陷入与系统调用分发、时钟中断 |
| 判分口径 | `check/ch5.py` 的 `EXPECTED_5` 中与第三章对应的 7 项（`EXPECTED_3` 去掉 `trace` 两项后为 5 项） |

> 报告编号与章节的对应关系由 checker 的 `Makefile` 决定：`CHAPTER=5 → LAB=3`，
> 即本分支需要 `lab1`、`lab2`、`lab3` 三份。`lab1` 记第三章地基，`lab2` 记第四章地址空间，
> `lab3` 记第五章的 `spawn` 与 stride 调度。

## 1. 本分支删掉了什么

`os/src/main.rs:102` 的 `rust_main()` 只做

```
clear_bss → kernel_log_info → mm::init → mm::remap_test
→ task::add_initproc → trap::init → enable_timer_interrupt
→ timer::set_next_trigger → loader::list_apps → task::run_tasks
```

没有 `batch::init()`。第三章的批处理子系统仍留在 `os/src/batch.rs`，但 `main.rs`
的模块列表（`:36-46`）里没有 `mod batch;`，所以它是一块不参与编译的死文件：
单道程序「装载下一个 app 前把应用区清零 + `fence.i`」的做法已经被地址空间机制取代。

## 2. 任务层的三个组成部分

- **`TaskControlBlock`**（`os/src/task/task.rs:15`）：不变部分 `pid: PidHandle`、
  `kernel_stack: KernelStack`，可变部分收在 `inner: UPSafeCell<TaskControlBlockInner>`。
  第一章到第五章都只有这一层，没有 `ProcessControlBlock`/线程概念，
  所以 `getpid()`（`:231`）返回的就是调度单位本身。
- **就绪队列**（`os/src/task/manager.rs:8`）：`TaskManager { ready_queue: VecDeque<Arc<..>> }`，
  对外包装 `add_task`（`:55`）、`fetch_task`（`:61`）。
- **CPU 侧**（`os/src/task/processor.rs:16`）：`Processor { current, idle_task_cx }`。
  `run_tasks()`（`:55`）是唯一的调度循环：取到任务就把 `task_status` 置 `Running`、
  先手动 `drop(task_inner)` 与 `drop(processor)` 再 `__switch(idle, next)`；
  取不到任务时打印 `warn!("no tasks available in run_tasks")`（`:74`）继续自旋。

控制流交接的前提是「切换期间不得持有 `RefMut`」，本分支用显式 `drop` 保证这一点，
`__switch` 只搬 `TaskContext` 的 6 个 `ra/sp/s` 槽位（`os/src/task/kswitch.S`）。

## 3. 陷入路径与用户态寄存器

`os/src/trap/trap_context.rs` 的 `TrapContext` 共 32 个 `usize`：`status/sp/ra` + `x[32]`。
它固定放在 `TRAP_CONTEXT_BASE = TRAMPOLINE - PAGE_SIZE`（`os/src/config.rs:19`），
用户页表里这一段由 `MemorySet::from_elf` 尾部映射（只带 `R | W`，不带 `U`），
因此 trampampoline 里的保存/恢复代码用同一套地址，陷入时无需再算。

`trap_handler`（`os/src/trap/mod.rs`）在返回用户态前检查当前任务状态，
若已 `Zombie` 就 `exit_current_and_run_next`，这是 `ch5_exit0/exit1` 那类
「子进程先退出、父进程 `wait`」能够收尾的原因。

## 4. 时钟

`os/src/timer.rs`：`TICKS_PER_SEC = 100`，`get_time()` 数 sbi timer tick，
`get_time_ms()`/`get_time_us()`（`:27`）把 `time::read()` 的 freerun 计数按
`CLOCK_FREQ = 12_500_000`（`os/src/config.rs:21`）换算。每次 `trap_handler` 收到
`Interrupt` 就 `set_next_trigger()` 续一次定时，并让当前任务 `suspend_current_and_run_next`，
这就是时间片轮转的来源。

第五章的 `set_priority` 与 stride 调度**不读时钟**：公平性按「被调度的次数」计，
见 `lab3`。

## 5. 判分契约（第三章部分）

| 判分项 | 来源测例 | 依赖 |
| --- | --- | --- |
| `get_time OK! <n>` | `ch3_sleep` | `sys_get_time` + `user_lib::sleep` 忙等循环 |
| `Test sleep OK!` | `ch3_sleep` | 同上 |
| `current time_msec = <n>` | `ch3_sleep1` | `sys_get_time` |
| `time_msec = <n> after sleeping <k> ticks, delta = <m>ms!` | `ch3_sleep1` | `sys_get_time` 的微秒字段 |
| `Test sleep1 passed!` | `ch3_sleep1` | 两次读数差值落在允许区间 |

`ch3_trace`/`ch4_trace1` 需要的 `Test trace OK!`、`Test trace_1 OK!`、
`string from task trace test` 在 `check/ch5.py` 里被显式从 `EXPECTED_5` 中减去，
本分支因此不实现 `trace`；`syscall/mod.rs:47` 的分发表也没有 `SYSCALL_TRACE`。

## 6. 需要注意的一处坑

`user_lib::sleep`（`ci-user/user/src/lib.rs`）是**用户态忙等**：
`let start = get_time(); while get_time() < start + period { sys_yield() }`，
而 `get_time()` 在 `sys_get_time` 返回非零时给出 `-1`。也就是说第五章骨架里
`sys_get_time` 那个 `NOT IMPLEMENTED` 桩一旦不补，`Test sleep OK!` 会把整个
usertest 拖到 30 秒 qemu 超时，看上去像「第三章没做完」，实际缺的是第五章的桩。
补齐过程与读数记录在 `lab3.md` 的复跑记录里。
