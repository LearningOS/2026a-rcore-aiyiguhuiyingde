# lab2 · 第四章：地址空间与 `mmap`/`munmap`（以 `ch5` 分支代码为准）

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch5` |
| 主题 | `MemorySet`/`MapArea`、SV39 页表按需建立、匿名映射与整段撤销、`sbrk` |
| 判分口径 | `EXPECTED_5` 中的 `Test 04_1 OK!`、`Test 04_4 test OK!`、`Test 04_5 ummap OK!`、`Test 04_6 ummap2 OK!`，以及不得出现 `Should cause error, Test 04_2 fail!`、`Should cause error, Test 04_3 fail!`、`Test sbrk failed!` |

## 1. 地址空间的数据结构

`os/src/mm/memory_set.rs:34`：

```rust
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}
```

`page_table` 是唯一真相，`areas` 是它的**簿记**：每个 `MapArea`（`:360`）记住
`vpn_range`、`data_frames: BTreeMap<VirtPageNum, FrameTracker>`、`map_type`、`map_perm`。
两者必须同步，否则要么帧泄漏，要么「内核以为还映射着、用户却已经读不到」。
本实验的 `munmap` 之所以拒绝部分撤销，就是这个原因（见 §4）。

`MemorySet::from_elf`（`:215`）按 program header 建 `Framed` 区，随后依次补：
守护页之后的用户栈（`:256`）、一个长度为 0 的占位区（`:266`，`sbrk` 用它作起点）、
`[TRAP_CONTEXT_BASE, TRAMPOLINE)` 的陷入上下文区（`:276`）。trampoline 由
`map_trampoline`（`:141`）单独直接写页表项，故意不进 `areas`。

## 2. 两个辅助判断：`perm_from_prot` 与 `mmap_span`

`os/src/syscall/process.rs:131` 把 `PROT_READ|WRITE|EXEC` 三个位翻译成
`MapPermission`，规则是「三位的子集才收下」：

| 输入 `port` | 结果 | 理由 |
| --- | --- | --- |
| `0` | `None` | 权限为空的映射没有意义，`04_4` 直接要求 `-1` |
| 含 3 位以外的位（例如 `prot|8`） | `None` | 不接受课程没定义的位 |
| 只给 `WRITE` | `None` | SV39 不允许「可写不可读」的页 |
| `R`/`W`/`X` 的合法组合 | `MapPermission::U` 加对应位 | 用户态可见 |

`os/src/syscall/process.rs:161` 的 `mmap_span` 负责算出这段区间真正覆盖到哪个页号：
`len == 0` 或 `start` 未按页对齐一律拒绝；`len` 向上取整到整页；
用 `checked_add`/`checked_mul` 防溢出；末端落在 `TRAMPOLINE` 及以上也拒绝，
避免用户把陷入上下文或 trampoline 抢走。

## 3. `sys_mmap`：先问「这段空着吗」

`os/src/syscall/process.rs:178`：

```rust
let (perm, end) = match (perm_from_prot(port), mmap_span(start, len)) { ... };
let mut inner = task.inner_exclusive_access();
if !inner.memory_set.is_range_free(from_va, to_va) { return -1; }
inner.memory_set.insert_framed_area(from_va, to_va, perm);
0
```

三个要点：

1. **返回值是 `0` 而不是地址**。`ch4_mmap0` 写死 `assert_eq!(0, mmap(0x10000000, 4096, 3))`，
   所以这里不能仿 Linux 返回 `start`。
2. **必须先查空**。`MemorySet::insert_framed_area`（`memory_set.rs:52`）继承的是
   「假定无冲突」的语义，`MapArea::map_one`（`:391`）在 `Framed` 分支里
   `data_frames.insert()` 覆盖旧 `FrameTracker`，旧帧的 `Arc` 计数被提前打下去——
   与其让内核静默做坏事，不如把重叠请求判成失败。检查由新方法
   `is_range_free`（`:81`）完成：逐页 `page_table.translate(vpn)`，只要
   `pte.is_valid()` 就算被占用；三级页表没建到那一层时 `translate` 返回 `None`，
   正好等价于「空」。
3. `port` 与区间的合法性都属于**参数校验**，不需要拿到 TCB 就能判，所以放在
   `inner_exclusive_access()` 之前，错误请求不会去抢全局锁。

## 4. `sys_munmap`：只撤销「完整的一组 `MapArea`」

`os/src/syscall/process.rs:200` 与 `mmap` 的取整规则**故意不一样**：
`len` 必须是整页、`start` 必须页对齐、末端不得触及 trampoline，否则 `-1`。
真正的撤销交给新方法 `MemorySet::unmap_range`（`memory_set.rs:98`），它的判定是：

- 收集所有**完全落在** `[from, to)` 内的 area；
- 一旦有 area 与请求区间部分重叠，立刻返回 `false`；
- 把收集到的 area 页数求和，若 `!= to.0 - from.0`，说明请求里还有不属于任何 area 的
  空页，同样返回 `false`；
- 三道关卡都过了，才按倒序 `areas[idx].unmap(&mut page_table)` 再 `remove(idx)`，
  倒序是为了让前面算出的下标在删除过程中不失效。

`MapArea::unmap`（`:417`）逐页 `unmap_one`：从 `data_frames` 移除 `FrameTracker`
（帧随 `Drop` 归还分配器）并清掉页表项。这样页表与簿记保持一一对应，
`from_existed_user`（`:292`）在 `fork` 时复制的 area 列表才不会出现「有洞」的区间。

对应到测例：

| 测例 | 请求 | 期望 | 命中规则 |
| --- | --- | --- | --- |
| `04_5` | `munmap(start, len)`（恰好一个 area） | `0` | 整组命中，页数相等 |
| `04_6` | `munmap(start, len + 1)` | `-1` | 请求含不属于任何 area 的页 |
| `04_6` | `munmap(start + 1, len - 1)` | `-1` | `start`/`len` 未页对齐 |

`mmap` 一侧则相反：`04_5` 里 `mmap(start - len, len + 1)` 要求 `0`，
靠的就是 §2 的「`len` 向上取整」——多出来的那一个字节把前一页也圈了进来。

## 5. `sbrk` 与堆区

`sys_sbrk`（`syscall/process.rs:221`）走 `TaskControlBlock::change_program_brk`
（`task/task.rs:249`），内部调 `MemorySet::shrink_to`/`append_to`（`memory_set.rs:331`、
`:346`），这两个方法操作的是**单个 area 的 `vpn_range`**，与 §4 的整组撤销是两套机制：
堆是连续增长的，`from_existed_user` 复制时按 `vpn_range` 逐页拷贝，所以 `append_to`
留的洞不会出现在堆上。判分只要求不打印 `Test sbrk failed!`，本分支的 sbrk 是完整实现。

## 6. 帧的收支

`04_5` 连着做 `mmap → mmap → munmap → mmap`，若 `munmap` 不还帧，
`frame_alloc()` 会在最后一个 `mmap` 处返回 `None` 并触发
`MapArea::map_one` 的 `unwrap()` panic。实测没有 panic，说明 `FrameTracker` 的
`Drop`（`os/src/mm/frame_allocator.rs`）确实把帧还回了 `FRAME_ALLOCATOR` 的空闲表。
