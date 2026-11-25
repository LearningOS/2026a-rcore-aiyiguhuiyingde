# lab3 · 第六章（下）：进程族在「程序搬到磁盘」之后怎么写

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch6` |
| 主题 | `exec`/`spawn`/`fork`/`wait(pid)` 与 `fd_table`，以及本章不需要实现的那一项 |
| 判分口径 | `EXPECTED_6` 中属于本篇的 7 项：`Test spawn0 OK!`、`Test wait OK!`、`Test waitpid OK!`、`exit pass.`、`forktest pass.`、`hello child process!`、`child process pid = (\\d+), exit code = (\\d+)` |
| 代码位置 | `os/src/syscall/process.rs:40/55/71/240`、`os/src/task/task.rs:103/154/181` |

## 1. 本分支的 TCB 只有一层

第五章分支上 `TaskControlBlock` 里套着 `ProcessRef`（进程与线程分离的写法），
本分支回到教程的形状：`TaskControlBlockInner` 直接持有
`memory_set`、`fd_table`、`children`、`parent: Option<Weak<_>>`、`exit_code`
与堆顶两个指针（`task/task.rs:41`-`:74`），`fd_table` 在 `:67`。
报告里所有行号都取自本分支，两章的实现不要混着引。

`fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>` 是第六章文件系统的入口：
`open` 把 `Arc<OSInode>` 塞进 `alloc_fd()`（`:89`）找到的空位，
`close` 只是 `take()` 掉那格——真正的资源释放靠 `Arc` 计数归零，
这也是 lab1 §4 里「`close` 只丢 `Arc`」的下一半。

## 2. `exec` 换了程序从哪里来

```rust
// os/src/syscall/process.rs:55
if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
    let all_data = app_inode.read_all();
    current_task().unwrap().exec(all_data.as_slice());
```

第五章分支靠 `loader::get_app_data_by_name`，那是编译期通过 `link_app.S`
连进内核镜像的一张表；本章把程序放进了 `fs.img`（lab1 §1），于是 `exec`
与 `open` 走的是同一条 easy-fs 路径，`read_all`（`fs/inode.rs:41`）按 512
字节一块地读到 `len == 0` 为止。内核镜像里不再有 ELF，代价是每次 `exec`
都要读一遍盘，收益是换程序不必重编内核。

`TaskControlBlock::exec`（`task.rs:154`）本身没变：重建 `memory_set`、
换 trap_cx、把 `fd_table`/`children`/`parent` 留着——这就是「同一进程换_program_」
与「造一个新进程」的差别，也正是 `spawn` 不能等于 `fork + exec` 的原因。

## 3. `spawn`：直接从 ELF 造一个 TCB

`ch5_spawn0` 连开 40 个子进程，`ch5_spawn1` 要求 `wait`/`waitpid` 拿到
66778 与 -233 两个退出码，所以父子关系必须建得和 `fork` 一样真。

`process.rs:240`：

```rust
let elf_data = app_inode.read_all();
let parent = current_task().unwrap();
let child = Arc::new(TaskControlBlock::new(elf_data.as_slice()));
let child_pid = child.pid.0;
child.inner_exclusive_access().parent = Some(Arc::downgrade(&parent));
parent.inner_exclusive_access().children.push(Arc::clone(&child));
add_task(child);
child_pid as isize
```

`TaskControlBlock::new`（`task.rs:103`）本来是给 `initproc` 用的，
它的语义恰好就是「按 ELF 建一个全新的地址空间并准备好陷入上下文」，
`spawn` 只是替它补上 `parent`/`children` 两条边。对照 `fork + exec`：

| 步骤 | `fork` 后 `exec` | `spawn` |
| --- | --- | --- |
| 地址空间 | 逐页复制父进程（`from_existed_user`，`task.rs:181` 内），随即在 `exec` 里整块丢弃重建 | 只按 program header 建一次 |
| 帧的收支 | 复制出来的帧立刻全部还掉，`40` 次 spawn 的量级上是一次白做的往返 | 一次分配 |
| `fd_table` | 继承父进程的每一项 | 重新得到 `stdin/stdout/stderr`（`task.rs:128`） |
| 谁被记进 `children` | 两个进程都记 | 记一次 |
| `syscall` 次数 | 2 | 1 |

多出来的那一层意思在 `sys_waitpid`（`process.rs:71`）的断言上：
从 `children` 里摘掉子任务后 `assert_eq!(Arc::strong_count(&child), 1)`。
`fork + exec` 那条路上，`exec` 之前 `trap_cx` 的拷贝、`task_cx` 的接管都可能
多留一份引用，而这里只有 scheduler 队列与父进程链表两处——`ch5_spawn1`
连续两次 `spawn` 再 `wait` 都通过了这条断言（实测 `Test wait OK!`、
`Test waitpid OK!` 两行都在输出里），说明引用点确实是干净的。

顺带说明 `fd_table` 在两条路上的不同后果：`fork` 是按 `Arc` 克隆
（`task.rs:195`-`:202`），父子拿到的是**同一个 `OSInode`**，
于是 lab1 §3 里那个「`OSInode` 只记 offset」变成了可观察语义——父子共享读写位置；
`spawn` 出来的孩子则从零开始，只有 0/1/2。测例没有直接压这一条，
但它是「为什么 `spawn` 不等于 `fork + exec`」最实在的一个理由。

## 4. 本章不实现的那一项

`sys_set_priority`（`process.rs:258`）保持骨架的 `-1` 桩。理由不是省事，
而是判分口径与本章测例表都不需要它：

- `check/ch6.py` 最后一步是 `EXPECTED_6 = set(EXPECTED_5 + EXPECTED_6b + 4 项) - set(["Test set_priority OK!"])`，
  优先级那一项被显式减掉了；
- `ch6_usertest` 的 `TESTS`（27 项）里没有 `ch5_setprio`、也没有 `ch5_stride0..5`。

所以本分支的调度器仍然是骨架的 FIFO（`task/manager.rs:25` 的
`ready_queue.pop_front()`）。虚拟时间（stride）调度器留在第五章分支上，
那里有六个 `stride` 测例要求它，两章各按各的口径实现，
不做「本章不考但顺手塞进来」的改动。

## 5. 实测读数

同一次 qemu 运行里（`oscamp2/logs/c_ch6_link1.log`），本篇相关的输出依次是：

```
488: Usertests: Running ch5b_exit           494: exit pass.
496: Usertests: Running ch5b_forktest_simple
500: hello child process!                   501: child process pid = 2, exit code = 100
649: Usertests: Running ch5_spawn0          730: Test spawn0 OK!
732: Usertests: Running ch5_spawn1          734: Test wait OK!    735: Test waitpid OK!
```

`ch5b_forktest` 的 `forktest pass.` 在 584 行（40 个 fork 之后）。
判分侧对应 774/780/782/791/794/797/798 七行 `[PASS]`。

27 个子测例（日志里 `Usertests: Running ch` 共 27 行）串在 `ch6_usertest` 的
`spawn` + `waitpid(pid[i])` 循环里，整套在 `overwrite/Makefile-ch6:23` 的
`timeout --foreground 30s qemu-system-riscv64` 之内自然结束：
日志里 `Error 124`（timeout 杀掉子进程时 make 会打的返回码）出现 0 次，
也没有 `panic`。
