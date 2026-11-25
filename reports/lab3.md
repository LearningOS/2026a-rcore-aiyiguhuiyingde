# lab3 · rCore 第八章分支上的进程与线程管理

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch8`（进程/线程机制在本分支为完整可用状态，报告按分支实际代码记录） |
| 主题 | 多进程与多线程、进程树、FIFO 就绪队列、退出码回收 |
| 判分口径 | `check/ch8.py` 的 `EXPECTED_8`（= `EXPECTED_6b` + 11 条并发项 − `Test sbrk almost OK!`），分母 25 |

## 1. 本章分支的判分边界（先说清不做什么）

`EXPECTED_8` 里**没有**第五章的练习项：`Test spawn0`、`Test set_priority` 只出现在 ch5 的期望集。
因此本分支的 `sys_spawn`（`os/src/syscall/process.rs:196`）与 `sys_set_priority`（`:207`）保持课程
模板桩——打印 `NOT IMPLEMENTED` 后返回 `-1`。这两项的实现属于 `ch4`/`ch5` 分支的交付物，
本报告不把它们算作本分支完成度。

## 2. 三层数据结构

第四章用 `Vec<TaskControlBlock>` + 一个 `current_task` 下标管理任务；本分支拆成三层：

- `ProcessControlBlockInner`（`os/src/task/process.rs:27`）：`is_zombie`、`memory_set`、
  `parent: Option<Weak<..>>`、`children: Vec<Arc<..>>`、`exit_code`、`fd_table`、`signals`、
  `tasks: Vec<Option<Arc<TaskControlBlock>>>`，外加 `deadlock_detect`（lab5 用）。
  `alloc_fd()`（`:62`）先复用空洞再扩容；`alloc_tid()`（`:71`）走 `task_res_allocator`。
- `TaskControlBlock`：`tid`、`task_status`、内核栈、`trap_cx_ptr`、`res`；线程退出时资源边由
  `forget()` 清除（lab5）。
- 全局表（`os/src/task/manager.rs:55-61`）：`TASK_MANAGER`（FIFO 就绪队列 + `stop_task`）与
  `PID2PCB: BTreeMap<usize, Arc<ProcessControlBlock>>`，配套 `pid2process()`（`:98`）、
  `insert_into_pid2process()`（`:104`）、`remove_from_pid2process()`（`:109`）。

`pid` 与 `tid` 必须分开：`ch8_deadlock_sem1` 直接按 `gettid()` 的 1/2/3 分支决定抢哪把资源，
所以 tid 由进程内的资源分配器递增发放（`os/src/task/id.rs`）。

## 3. 调度：FIFO 队列 + `run_tasks()` 集中处理

就绪队列在 `os/src/task/manager.rs`：`add()`（`:29`）`push_back`、`fetch()`（`:33`）`pop_front`、
`remove()`（`:36`）按 `Arc::as_ptr` 比较摘除、`add_stop()`（`:47`）保存「刚被杀但还在用内核栈」
的任务，注释明确说明单核下可以直接覆盖上一个 `stop_task`。
模块级 `add_task`/`wakeup_task`/`remove_task`/`fetch_task`/`add_stopping_task`（`:66-:93`）
是队列的对外包装，`run_tasks()`（`os/src/task/processor.rs:53`）负责从队列取任务、必要时
`__switch`，`current_process()`（`:88`）给系统调用提供当前 PCB 弧。

这样「阻塞—唤醒—回收」不需要每个系统调用各自实现一次切换，第四章那种散点式
`suspend_current_and_run_next()` 在这里收敛成一个入口。

## 4. 系统调用要点（逐条对照代码）

- `sys_fork`（`syscall/process.rs:44`）：`current_process.fork()` 生成子 PCB，随后取子进程
  `tasks[0]` 的 `TrapContext` 把 `x[10]` 置 0——子进程从同一 `sepc` 继续但看到返回值 0，
  父进程看到子 `pid`。
- `sys_exec`（`:62`）：`translated_str` 取路径，再对 `args` 指针数组逐项 `translated_ref` +
  `translated_str` 直到遇到空指针；`open_file(path, OpenFlags::RDONLY)` 成功后
  `app_inode.read_all()` 拿到 ELF 字节，交给 `process.exec(all_data, args_vec)`，
  返回 `argc`。取路径与取参数都经过页表翻译，不直接解引用用户指针。
- `sys_waitpid`（`:96`）：先看 `children` 里有没有匹配 `pid`（`-1` 表示任意子进程），没有返回
  `-1`；有但不是 `is_zombie` 则返回 `-2`（测试侧的 `wait`/`waitpid` 靠这个返回值轮询）；
  找到僵尸就 `children.remove(idx)`，`assert_eq!(Arc::strong_count(&child), 1)` 确认没人再引用，
  然后把子进程 `exit_code` 经 `translated_refmut` 写回用户指针，返回子 `pid`。
- `sys_kill`（`:132`）：信号记录在 `SignalFlags`（`os/src/task/signal.rs`），`SIGKILL` 走立即终止，
  其余信号只置标志位。`os/src/task/action.rs` 提供的是 `SignalAction`/`SignalActions`
  （处理函数地址 + 掩码表），不是任务调度动作枚举——这点与部分教程版本不同，读代码时容易误判。

## 5. 与判分集的对应

`EXPECTED_8` 中由进程/线程机制支撑的判决项：`threads test passed!`、
`threads with arg test passed!`、`test_condvar passed!`、`mpsc_sem passed!`、`sync_sem passed!`、
`race adder using spin mutex test passed!`、`philosopher dining problem with mutex test passed!`、
`pipetest passed!`（管道依赖 `fd_table`/`alloc_fd()`，见 lab4）。

ch8 基线轮 `logs/c_ch8_base2.log` 读数 `Test passed3524: 22/25`，缺的三条全部是同步层
（`deadlock test mutex 1 / semaphore 1 / semaphore 2`），与进程管理无关，根因与修法记入 lab5。
