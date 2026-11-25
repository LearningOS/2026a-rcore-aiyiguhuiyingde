# lab3 · 第五章：`spawn`、优先级与 stride 调度（以 `ch5` 分支代码为准）

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch5` |
| 主题 | 直接从 elf 建子进程地址空间、进程树与回收、`set_priority`、stride（彩票/虚拟时间）调度 |
| 判分口径 | `EXPECTED_5` 新增的 `Test spawn0 OK!`、`Test wait OK!`、`Test waitpid OK!`、`Test set_priority OK!`，以及 `ch5_stride` 必须正常收尾（`Usertests passed!`） |

本分支的第五章一共留了 5 个 `YOUR JOB` 桩：`sys_get_time`、`sys_mmap`、`sys_munmap`、
`sys_spawn`、`sys_set_priority`。前两个属于第四章口径（见 `lab2.md`），
`sys_get_time` 的连带影响见 §5，本文件记 `spawn` 与调度。

## 1. `sys_spawn`：`fork + exec` 不等于 `spawn`

`os/src/syscall/process.rs:237`：

```rust
let elf_data = match get_app_data_by_name(path.as_str()) { Some(d) => d, None => return -1 };
let parent = current_task().unwrap();
let child = Arc::new(TaskControlBlock::new(elf_data));
let child_pid = child.pid.0;
child.inner_exclusive_access().parent = Some(Arc::downgrade(&parent));
parent.inner_exclusive_access().children.push(Arc::clone(&child));
add_task(child);
child_pid as isize
```

对比 `fork` 再 `exec` 的三步开销：`sys_fork`（`:41`）会先 `TaskControlBlock::fork`
（`task/task.rs:178`）→ `MemorySet::from_existed_user` 逐页分配新帧并整页拷贝父进程
地址空间，紧接着 `sys_exec`（`:56`）又用 `MemorySet::from_elf` 把这套刚拷完的帧全部
丢掉。`spawn` 只需要最终状态，所以直接让 `TaskControlBlock::new(elf_data)` 建空间，
从父进程那里只继承**父子关系**这一件事。

两个细节：

- `TaskControlBlock::new` 已经写好 `TrapContext::app_init_context(entry_point, user_sp, ..)`
  （`task/task.rs:135` 之后），子进程从自己的 `trap_return` 起跑，
  因此**不需要**像 `sys_fork` 那样把 `trap_cx.x[10]` 改成 `0`。
- `parent` 存的是 `Weak`（`:58`），`children` 存 `Arc`：这样子进程退出时
  `sys_waitpid` 里 `assert_eq!(Arc::strong_count(&child), 1)`（`:94`）才成立，
  回收即释放。

`ch5_spawn0` 连开 40 个 `ch5_getpid` 子进程再 40 次 `wait`，最后一次 `wait` 必须 `<= 0`。
40 次 spawn 会不会耗尽 `pid_allocator`？——不会：每个子进程被 `wait` 从 `children` 里摘走、
`Arc` 计数归一后 `Drop`，`impl Drop for PidHandle`（`os/src/task/id.rs:53`）把号
`dealloc` 回 `recycled` 队列。`dealloc`（`:36`）里那句
`assert!( !self.recycled.iter().any(|i| *i == id), "id {} has been deallocated!" )`
顺带充当「同一个孩子被收割两次」的检测器，这正是回收链路要判的点。

`ch5_spawn1` 则验返回值语义：`spawn("ch5_exit0")` → `wait` 拿到 `(cpid, 66778)`；
再 `spawn("ch5_exit0")` + `spawn("ch5_exit1")`，用 `waitpid(cpid1)` 精确取到 `-233`，
剩下的 `cpid0` 由随后的 `wait` 收走。`sys_waitpid` 的三态约定（没有这个孩子 `-1`、
孩子还在跑 `-2`、收割返回 pid）在本分支是原有实现，未改动。

## 2. `sys_set_priority`：合法区间与拒绝条件

`os/src/syscall/process.rs:259`：

```rust
if prio <= 1 { return -1; }
current_task().unwrap().set_priority(prio as usize);
prio
```

`ch5_setprio` 的断言逐条对应：

| 调用 | 期望返回 | 说明 |
| --- | --- | --- |
| `set_priority(10)` | `10` | 正常接受 |
| `set_priority(isize::MAX)` | `isize::MAX` | 极大值也要接受，见 §3 的下限保护 |
| `set_priority(0)` | `-1` | `0` 会让除法非法 |
| `set_priority(1)` | `-1` | `1` 已经是默认份额，课程把它当无效请求 |
| `set_priority(-10)` | `-1` | 负数按无符号解释会是个天文数字，必须在 `isize` 阶段挡掉 |

## 3. stride 调度：把「份额」记在任务身上

新增常量 `BIG_STRIDE = 1 << 20`（`os/src/config.rs:25`）。
`TaskControlBlockInner` 加三个字段（`os/src/task/task.rs:73`、`:76`、`:79`）：

| 字段 | 含义 | `new` | `fork` |
| --- | --- | --- | --- |
| `priority` | 用户请求的份额 | `1` | 继承父值（`task.rs:192`） |
| `stride` | 每次被调度要付的虚拟时间 | `BIG_STRIDE` | 继承父值 |
| `pass` | 已经付过的虚拟时间 | `0` | 继承父值 |

`TaskControlBlock::set_priority`（`task.rs:242`）改写 stride：

```rust
inner.stride = (BIG_STRIDE / priority).max(1);
```

`.max(1)` 不是凑数：`set_priority(isize::MAX)` 是被判分接受的，
`BIG_STRIDE / isize::MAX == 0`，一旦 stride 为 `0`，这个任务的 `pass` 永远涨不起来，
它会独占处理器把别人饿死。取 `1` 之后它顶多是「每轮都被选中」，不会破坏簿记。

`TaskManager::fetch`（`os/src/task/manager.rs:32`）从 `pop_front` 换成：

```rust
for (idx, task) in self.ready_queue.iter().enumerate() {   // 找最小 pass
    let pass = task.inner_exclusive_access().pass;
    if best.map_or(true, |(_, lowest)| pass < lowest) { best = Some((idx, pass)); }
}
let (idx, _) = best?;                                      // 空队列 -> None
let task = self.ready_queue.remove(idx).unwrap();
let stride = task.inner_exclusive_access().stride;
task.inner_exclusive_access().pass += stride;               // 立刻记账
```

要点：

1. 记账放在**取出之后**，所以「谁被选中」只看历史用量，优先级高的任务 `pass`
   涨得慢，自然被更频繁地选中——`count/priority` 因此大致相等。
2. `run_tasks()`（`task/processor.rs:55`）对 `fetch_task()` 返回 `None` 的处理是
   `warn!` 后继续自旋，`best?` 直接对应这个约定，不需要额外分支。
3. 两次 `inner_exclusive_access()` 是顺序发生的（先读 `stride` 到局部变量，
   再改 `pass`），不会同时持有两个 `RefMut`。
4. `fork` 继承 `pass` 而不是清零：新子进程不应获得「从零开始」的额外便宜，
   否则 `ch5_spawn0` 的 40 连 spawn 会让子代反复插队。

`ch5_stride` 的 6 个子进程优先级为 `5/6/7/8/9/10`，主进程自己 `set_priority(4)`，
每个进程自旋到 `get_time() - start > 4000ms` 后输出 `priority/count/ratio`。
ratio 不参与判分，判分只要求这段能在 qemu 的 30 秒窗口里跑完并回到
`ch5 Usertests passed!`。

## 4. 与第四章口径的接口

`ch5_usertest` 现在**全部用 `spawn` 起跑测例**（`ci-user/user/src/bin/ch5_usertest.rs`
里的 `spawn(test)` + `waitpid(pid[i])`），而不是老版本的一个个 `exec`。
所以 `sys_spawn` 只要返回 `-1`，后面的 `assert_eq!(pid[i], wait_pid)` 链条会整体崩掉，
第四章已经做完的 `04_1`/`04_5` 也会连带丢分——这就是本分支第五章「不动第四章代码、
分数却涨上去」的原因。

## 5. 一处必须一起补的遗留桩：`sys_get_time`

`ci-user/user/src/lib.rs` 的 `sleep()` 是用户态忙等：反复 `get_time()` 比较再 `sys_yield()`，
而 `get_time()` 在 `sys_get_time` 返回非 `0` 时给出 `-1`。第五章骨架把 `sys_get_time`
留成 `NOT IMPLEMENTED` 桩，于是 `ch3_sleep` 的死循环把整个 usertest 顶到 30 秒超时，
判分表上表现为「第三章、第四章项集体缺失」。

`os/src/syscall/process.rs:112` 的实现按字节走页表写回 16 字节 `TimeVal`
（`sec = us / 1_000_000`、`usec = us % 1_000_000`），一次 `translated_refmut` 一个字节，
这样结构体跨页时也不依赖对齐。

## 6. 复跑记录

本地判分尺子：worktree 克隆到隔离目录，pin 住 checker `7d61ec5` 与 tests `a059366`，
`make test CHAPTER=5`（学生 passwd 为空 ⇒ `BASE=2` ⇒ 走 `check/ch5.py`，分母 15）。

| 读数 | 结果 |
| --- | --- |
| 判决汇总 | `Test passed32659: 15/15` |
| `[FAIL]` 条数 | 0 |
| 逐条缺口（`rig_c2_gaps.py ch5`） | 需补齐 0，不该出现却出现了 0 |
| 报告闸 | `Report for lab1 found.` / `lab2` / `lab3` |
| `MAKE_RC` | 0 |
| 内核 panic / qemu 30 秒超时 | 均无（日志里 `panic`、`Error 124` 计数为 0） |

stride 的实际分布（`ch5_stride0..5` 的 `priority/count/ratio` 行，ratio = `count/priority`）：

```
priority = 5,  exitcode = 22070000, ratio = 4414000
priority = 6,  exitcode = 26976800, ratio = 4496133
priority = 7,  exitcode = 30563600, ratio = 4366228
priority = 8,  exitcode = 34648400, ratio = 4331050
priority = 9,  exitcode = 40210400, ratio = 4467822
priority = 10, exitcode = 45729600, ratio = 4572960
```

`count` 随 `priority` 单调上升，比值落在 `4.33M ~ 4.57M`（极差约 5%），
说明「按最小 `pass` 选中、选中后立刻按 `stride` 记账」确实把 CPU 按比例分了出去，
而不是只让高优先级插队。ratio 本身不参与判分，这里只作为机制正确的旁证。

第四章口径在同一轮里也全部通过：`Test 04_1 OK!`、`Test 04_4 test OK!`、
`Test 04_5 ummap OK!`、`Test 04_6 ummap2 OK!`，`NOT_EXPECTED` 的
`FAIL: T.T`、`Test sbrk failed!` 均未出现。

排查过程中确认的一件事：本轮之前 ch5 只有 2/15，缺失的并不只是第五章的 4 项，
而是 `Test sleep OK!`、`get_time OK!`、`Test 04_*` 一起消失——根因是 §5 的
`sys_get_time` 桩让忙等 `sleep()` 永不返回，usertest 卡死到 30 秒超时。
补 `sys_get_time` 属于第五章作业本身，但它救回的是第三章的分。

本轮判分未使用任何插桩代码：`git status` 中与判分相关的改动只有
`os/src/config.rs`、`os/src/mm/memory_set.rs`、`os/src/syscall/process.rs`、
`os/src/task/manager.rs`、`os/src/task/task.rs` 五个文件与 `reports/`。

