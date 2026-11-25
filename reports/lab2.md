# lab2 · 第六章（中）：时间与地址空间——`get_time`/`mmap`/`munmap`/`sbrk` 在本分支的落地

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch6` |
| 主题 | `TimeVal` 的跨页写出、匿名映射的准入判断、整段撤销、堆顶移动 |
| 判分口径 | 期望集（`check/ch6.py` 的 29 项）里属于本篇的这几项：`Test 04_1 OK!`、`Test 04_4 test OK!`、`Test 04_5 ummap OK!`、`Test 04_6 ummap2 OK!`、`Test sbrk almost OK!`、`get_time OK! (\d+)`、`Test sleep OK!`、`Test sleep1 passed!`、`current time_msec = (\d+)`、`time_msec = ... after sleeping ... ticks, delta = ...ms!`；`ch6` 的否定集只有 `FAIL: T.T` 与 `Test sbrk failed!` |
| 代码位置 | `os/src/syscall/process.rs:114/180/202/223`、`os/src/mm/memory_set.rs:87/105` |

第六章的镜像是从磁盘装载程序的（见 lab1），但地址空间这一套系统调用仍然是必答题：
`ch6_usertest` 的 27 个子测例里有 7 个属于本章之前的内容，
它们与文件测例共用同一次 qemu 运行，**任何一处返回 `-1` 都会连带拖垮整套**。

## 1. `sys_get_time`：先想清楚「谁来读这 16 个字节」

`TimeVal`（`process.rs:18`）是两个 `usize`，共 16 字节。骨架的提示是
「如果它被两个页切开怎么办」。这不是假想敌：用户测例里 `TimeVal` 是栈上的
局部变量，栈是页对齐映射的，但变量本身的地址可以落在页尾——
`translate_va` 只会翻译**首地址所在的那一页**，于是一次宽写要么写到
相邻物理帧的末尾就断掉，要么越界踩到别的数据。

`process.rs:114` 的写法是：在内核栈上把 `sec`/`usec` 按本机字节序摊平成
`[u8; 16]`，然后逐字节取地址、逐字节过页表：

```rust
for (offset, byte) in raw.iter().enumerate() {
    *translated_refmut(token, unsafe { base.add(offset) }) = *byte;
}
```

单字节访问天然不会跨页（`PAGE_SIZE % 1 == 0`），16 次翻译的开销对用户态
忙等的 `sleep` 来说完全可以忽略。**返回 `0` 是硬要求**：`user_lib` 的包装把返回值不当错误看，而是当数据看——

```rust
// ci-user/user/src/lib.rs:209
match sys_get_time(&mut time, 0) {
    0 => ((time.sec & 0xffff) * 1000 + time.usec / 1000) as isize,
    _ => -1,
}
```

非 0 就永远得到 `-1`，于是 `sleep` 的 `while get_time() < start + period_ms`（`ci-user/user/src/lib.rs:265`）
恒成立，用户态变成无限 `sys_yield` 打转；整套 usertest 会一直卡到判分 Makefile
的那条 30 秒 `timeout`（`overwrite/Makefile-ch6:23`，见 lab1 §1）把 qemu 杀掉为止。
顺带可见用户态只认 `sec` 的低 16 位，所以内核写回的 `sec` 只要如实递增就够用。

## 2. 参数校验放在拿到 TCB 之前

`mmap`/`munmap` 的合法性判断都不需要碰进程控制块，所以拆成两个纯函数，
让明显的错误请求连全局锁都不去抢：

| 函数 | 位置 | 接受 | 拒绝 |
| --- | --- | --- | --- |
| `perm_from_prot` | `process.rs:134` | `R`/`W`/`X` 三位的非空子集，且「可写必可读」 | `prot = 0`、含第 4 位以上（`prot \| 8`）、纯 `W` |
| `mmap_span` | `process.rs:164` | 页对齐的 `start`；`len` **向上取整**到整页 | `len = 0`、`start` 未对齐、末端 ≥ `TRAMPOLINE`、任何溢出（`checked_add`/`checked_mul`） |

`04_4` 的三条否定式断言分别命中这三行规则：
`mmap(start + len + 1, len, prot)` 因为 `start` 不对齐被拒；
`mmap(start + len, len, 0)` 与 `mmap(start + len, len, prot | 8)` 因为权限位被拒；
而 `mmap(start - len, len + 1, prot)` 在 `04_4` 里要求 `-1`、在 `04_5` 里要求 `0`——
差别不在取整规则（两处都要向上取整），而在**区间是否已被占用**，见下一节。

「可写必可读」不是洁癖：SV39 的 `PTEFlags` 里 `W` 位脱离 `R` 位没有意义，
`ch4_mmap2`（`prot = 2`，`ci-user/user/src/bin/ch4_mmap2.rs:17`）就是专门用来把这条逼出来的测例——它先
`assert_eq!(0, mmap(start, len, prot))`，一旦内核真给了一个只写页，它就会继续往下走、
一路打印到 `Should cause error, Test 04_2 fail!`（`ch4_mmap2.rs:24`，测例自己的编号笔误，
判分的 `NOT_EXPECTED` 里 04_2、04_3 两条都列着）；我们返回 `-1`，
它的断言当场 panic，那行文案不会出现——`c_ch6_final.log` 第 475 行
`Panicked at src/bin/ch4_mmap2.rs:18` 就是这条断言留下的痕迹。`ch4_mmap1`（`prot = 1`）相反：
只读映射必须成功返回 `0`，随后那次写要真的陷入页故障、进程被杀，
同样不许它打印 `Should cause error, Test 04_2 fail!`。
一个测例要求拒绝、一个要求接受，靠 `perm_from_prot` 里那两个分支就能分开。

要说清楚的是判分范围：这两行文案只出现在 `check/ch4.py:27`、`:30` 的 `NOT_EXPECTED_4` 里，
`ch6.py:22` 用的否定集是 `NOT_EXPECTED_4b`（定义在 `check/ch4b.py:10-13`，
= `NOT_EXPECTED_2b` + `Test sbrk failed!`，实测就是 `FAIL: T.T` 与 `Test sbrk failed!` 两项），
所以本章不判它们，本篇的实现是**按第四章的口径**保持正确的，而不是钻本章的空子。

## 3. `sys_mmap`：重叠一律判失败

`process.rs:180`：

```rust
if !inner.memory_set.span_is_free(from_va, to_va) { return -1; }
inner.memory_set.insert_framed_area(from_va, to_va, perm);
```

成功时答案是 `0`，不是映射地址——所有 mmap 测例写的都是
`assert_eq!(0, mmap(start, len, prot))`（`ci-user/user/src/bin/ch4_mmap0.rs:18`、
`ch4_mmap2.rs:18`、`ch4_unmap.rs:18`）。
本内核不替用户挑地址：`start` 由调用方给定，内核只需回答「映射成了」或「不给映射」。

`insert_framed_area`（`memory_set.rs:58`）继承的是骨架「假定无冲突」的语义，
`MapArea::map_one` 在 `Framed` 分支里对已有条目 `data_frames.insert()`，
被顶掉的 `FrameTracker` 当场 `Drop` 还帧——页表项还指着旧帧，
帧却已经回到分配器手里，这种「内核静默做坏事」的状态不能留给用户请求制造。
所以新增 `MemorySet::span_is_free`（`memory_set.rs:87`）：

```rust
VPNRange::new(start_va.floor(), end_va.ceil())
    .into_iter()
    .all(|vpn| self.page_table.translate(vpn).filter(|pte| pte.is_valid()).is_none())
```

`translate` 在三级页表还没建到那一层时返回 `None`，正好等价于「空」；
`filter(|pte| pte.is_valid())` 处理的是「表走到了但项没有效位」这一种情况。
每次成功 mmap 都会新建一个 `MapArea`，这个「一次调用一个区」的性质是下一节能整段撤销的前提。

## 4. `sys_munmap`：只撤销「完整的一组 area」

`process.rs:202` 与 mmap 的取整规则**故意不一样**：`len` 必须是整页、
`start` 必须页对齐、末端不得触及 trampoline，否则直接 `-1`——`04_6` 的
`munmap(start, len + 1)` 与 `munmap(start + 1, len - 1)` 就是这两条。

真正的撤销交给 `MemorySet::unmap_span`（`memory_set.rs:105`），三道关卡：

1. 收集所有**完全落在** `[from, to)` 内的 area；
2. 只要有 area 与请求区间部分重叠，立即 `false`——留下半个 area，
   `fork` 时按 `vpn_range` 逐页拷贝的簿记就会出现「有洞」的区间；
3. 把命中 area 的页数求和，若 `!= to.0 - from.0`，说明请求里还有不属于任何 area 的
   空页，同样 `false`——撤销一半却报告成功是撒谎。

三关都过了才按**倒序** `areas[idx].unmap(&mut page_table)` 并 `remove(idx)`，
倒序是为了让前面算出来的下标在删除过程中不失效。`MapArea::unmap` 逐页把
`FrameTracker` 从 `data_frames` 摘掉（帧随 `Drop` 归还）并清页表项，
`04_5` 连着做 `mmap → mmap → munmap → mmap` 而 `frame_alloc()` 从未返回
`None`（否则会 panic 在 `map_one` 的 `unwrap()`），说明帧确实还回去了。

## 5. `sys_sbrk`

本章骨架已经给了完整实现（`process.rs:223` → `TaskControlBlock::change_program_brk`，
`task/task.rs:240`），它操作的是**单个 area 的 `vpn_range`**（`shrink_to`/`append_to`），
与 §4 的「整组撤销」是两套机制：堆是连续增长的，不存在跨 area 的簿记问题。
判分只要求不出现 `Test sbrk failed!`，本分支 `ch4b_sbrk` 打印的是
`Test sbrk almost OK!`，属于期望集。

## 6. 与第五章分支的关系

同一条规则在两个分支上各写了一遍：`ch5` 分支的对应实现放在
`is_range_free`/`unmap_range` 里，本分支改名为 `span_is_free`/`unmap_span`
并按第六章的代码形状重写了一遍注释与判断顺序。两个分支各自独立编译、独立判分，
`ch6` 的这一份才是本章读数的来源——报告里的行号与结论都取自本分支。
