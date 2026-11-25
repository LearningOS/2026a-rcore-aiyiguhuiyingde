# lab5 · rCore 第八章：同步原语与死锁检测（本分支的实际改动）

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch8`（`LearningOS/2026a-rcore-aiyiguhuiyingde`） |
| 主题 | 阻塞式 mutex/semaphore 的死锁检测、`-0xdead` 错误码、wait-for 图 |
| 判分口径 | `check/ch8.py` 的 `EXPECTED_8`，分母 25；`BASE=2`（passwd 空）⇒ 执行 `ch8.py` 而非 `ch8b.py` |
| 尺子 | `bash oscamp2/rig_c2_run.sh ch8 <后缀>`（官方 `make test CHAPTER=8`，checker `7d61ec55`、tests `a0593662` 固定） |

## 1. 环境与判分基线

`rustc=rustc 1.80.0-nightly (c987ad527 2024-05-01) cargo=cargo 1.80.0-nightly (6087566b3 2024-04-30)
qemu=QEMU emulator version 11.1.3`；官方 CI 断言 QEMU 8.2.x，本机 11.1.3 的口径差此前已在
同版本 qemu 上实测关闭（同一套内核在 8.2.2 上同为满分）。

基线轮 `oscamp2/logs/c_ch8_base2.log`：

```text
Test passed3524: 22/25
MAKE_RC=2
```

`python3 oscamp2/rig_c2_gaps.py ch8 logs/c_ch8_base2.log` 的缺口清单精确到三条：

```text
需补齐=3
  MISS  deadlock test semaphore 2 OK3524!
  MISS  deadlock test semaphore 1 OK3524!
  MISS  deadlock test mutex 1 OK3524!
不该出现却出现了=0
```

即除死锁检测外，本分支的线程、同步、文件与批处理判决全部已经通过；`NOT_EXPECTED_4b` 四条
（`FAIL: T.T`、`Test sbrk failed!` 等）继续保持不出现。

## 2. 测试契约（从固定住的测试源码反解，不靠猜）

`rCore-Tutorial-Test/src/bin/ch8_deadlock_mutex1.rs`：

```rust
enable_deadlock_detect(true);
let mid = mutex_blocking_create() as usize;
assert_eq!(mutex_lock(mid), 0);
assert_eq!(mutex_lock(mid), -0xdead);   // 同一线程二次上锁
```

`ch8_deadlock_sem1.rs` 用 `RES_NUM = [1, 2, 1]`、`REQUEST = [Some(1), Some(3), Some(2)]` 造出
三线程环：线程 1 持 sem2 抢 sem1，线程 2 持 sem1+sem2 抢 sem3，线程 3 持 sem3 抢 sem2。
`try_sem_down()` 在 `semaphore_down() == -0xdead` 时 `exit(-1)`，主线程最后
`assert!(failed > 0)`——**只要环里任意一个线程被拒**即可，不需要全部拒绝。
syscall 号：`SYSCALL_ENABLE_DEADLOCK_DETECT = 469`、`SYSCALL_SEMAPHORE_DOWN = 470`。

模板缺的正是这两个入口：`sys_mutex_lock`/`sys_semaphore_down` 只会把任务挂到等待队列，
自锁或成环时内核与线程一起永远睡下去，30 秒 `timeout --foreground` 到点截断，判决自然拿不到。

## 3. 首轮失败：不是逻辑错，是 lint 拦住了编译

补完逻辑后第一次判分读数掉到 `Test passed50183: 2/25`，日志里是：

```text
error: missing documentation for an enum
  --> src/task/deadlock.rs:6:1
...
error: could not compile `os` (bin "os") due to 10 previous errors
```

`os/src/main.rs:21-22` 有 `#![deny(missing_docs)]` 与 `#![deny(warnings)]`：新增模块里每个
**对外可达**的项（`ResourceId` 及其两个变体、`set_detection`、`request_would_deadlock`、
`grant`、`release`、`hand_over`、`set_waiting`、`forget`，共 10 条）没有文档注释就构成硬错误，
内核编译失败 ⇒ 25 条判决只剩 `NOT_EXPECTED` 的 2 条。给 10 个公开项补上文档注释后：

```text
ch=ch8 check_rc=0
error_lines=0
warning_lines=0
```

（`oscamp2/rig_c2_check.sh ch8`，`cargo check --release`；用 check 而不是 build 是因为 worktree
里没有 app 二进制，`cargo build` 会在 `link_app.S` 上报假的 `undefined symbol`。）

## 4. 实现：以「这个请求还能等到进展吗」代替显式找环

新增 `os/src/task/deadlock.rs`，数据结构只有两个：

```rust
pub enum ResourceId { Semaphore(usize), Mutex(usize) }   // seq 即发给用户态的槽号
struct ThreadSnapshot { tid, held: Vec<ResourceId>, blocked_on: Option<ResourceId> }
```

`request_would_deadlock()` 在**入队阻塞之前**被调用：先取本进程所有 `Arc<TaskControlBlock>`
的快照（避免递归时反复独占 PCB），再问 `can_release(resource, requester, &snapshots, &mut path)`。
`can_release` 的语义是「`resource` 的某个持有者能不能跑到完成」：

- 持有者只要不是请求者本人，且它 `blocked_on == None`（在跑或在睡），就算它能推进 → 返回 `true`；
- 持有者自己也在等别的资源，就递归问那一个；`path` 记录已经问过的资源，重复即剪枝，
  因此环不会让递归跑飞；
- 过滤条件 `thread.tid != requester` 正好覆盖 mutex 自锁：唯一的持有者就是自己，
  过滤后集合为空 → `false` → 返回 `-0xdead`，与 `ch8_deadlock_mutex1` 的断言逐字对应；
- 三线程环里，每个请求沿 `held → blocked_on` 走一圈都会回到请求者，集合内没有 `None` 出口，
  所以至少一个线程拿到 `-0xdead`，满足 `assert!(failed > 0)`。

`semaphore` 用 `ResourceId::Semaphore(seq)` 作节点而不是「剩余计数」，所以 `res_count > 1`
的那把 sem2 在图上仍是单一节点：多个持有者会被 `filter` 全部列出，任一能推进就不算死锁，
这正是计数信号量该有的判断方式（不会把「还有名额」误判成死锁，也不会要求全部持有者都能跑）。

簿记接口分工：`set_waiting()` 在挂起前写下 `blocked_on` 边；`hand_over()`（唤醒方把资源移交
时）断言该边存在、清掉它并记一次所有权；`grant()`/`release()` 增减所有权（`release` 只去掉
一次持有，重复持有的线程在上锁一次后仍算持有者）；`forget()` 在线程退出时清空它的全部边与
所有权——漏掉这一步会让后继测试误判出假死锁。开关是进程级的：
`ProcessControlBlockInner::deadlock_detect`（`task/process.rs:52`），`sys_enable_deadlock_detect`
（`syscall/sync.rs:256`）只改这一个布尔，关闭时 mutex/semaphore 回到原来的纯阻塞路径，
所以 22 条已通过的并发判决不受影响。

改动落在 7 个文件：`os/src/task/deadlock.rs`（新增）、`os/src/task/mod.rs`（导出）、
`os/src/task/process.rs` 与 `os/src/task/task.rs`（进程开关、任务侧 `held_resources`/`blocked_on`）、
`os/src/sync/mutex.rs`、`os/src/sync/semaphore.rs`（阻塞前查图并回 `-0xdead`）、
`os/src/syscall/sync.rs`（469 入口）。

## 5. 复跑记录

判分读数与报告闸结果在本节补记：`Test passedNNN: n/25`、`Report for lab1..lab5 found.`、`MAKE_RC=`。
