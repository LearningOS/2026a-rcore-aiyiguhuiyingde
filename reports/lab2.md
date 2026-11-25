# lab2 · rCore 第四章：虚拟内存、用户指针与 mmap/unmap

| 项目 | 值 |
| --- | --- |
| 分支 | `ch4` |
| 账号 | `aiyiguhuiyingde` |
| 日期 | 2026-11-25 |
| 判分结果 | 代码判决 **16/16**；首次满分轮因缺 lab1/lab2 报告 `MAKE_RC=2`，补齐后复跑 |
| 判分日志 | `oscamp2/logs/c_ch4_takeover2.log` |

> 本报告只记录真实执行过的命令与日志。本机判分使用 Homebrew QEMU 11.1.3，课程 CI 使用
> QEMU 8.2.x；两者版本差异如实保留，不把本地结果称为 CI 结果。

## 1. 环境与判分基线

判分日志首部：

```text
### rustc=rustc 1.80.0-nightly (c987ad527 2024-05-01) cargo=cargo 1.80.0-nightly (6087566b3 2024-04-30) qemu=QEMU emulator version 11.1.3 timeout=/usr/bin/timeout
### checker=7d61ec55b58eed6ca7052917846c1b87af34563a tests=a0593662ad55d670ba8c27ce1763347cd0dd552f
```

学生侧 `passwd` 为空，因此实际执行 `check/ch4.py`，分母为 16：12 条期望输出加 4 条禁止输出。
判分器注入的随机数字（本轮为 `18890`）只用于防止硬编码，不属于实现。

## 2. 初始失败与根因

上一轮基线 `oscamp2/logs/c_ch4_scratch.log` 为 `4/16`，且 4 条全部来自 `NOT_EXPECTED`，说明
第四章的真实功能一条都没有通过。关键原始行：

```text
Panicked at src/bin/ch3_sleep.rs:16, assertion failed: current_time > 0
Panicked at src/bin/ch4_mmap0.rs:18, assertion `left == right` failed
Panicked at src/bin/ch4_mmap1.rs:18, assertion `left == right` failed
Panicked at src/bin/ch4_mmap2.rs:18, assertion `left == right` failed
Panicked at src/bin/ch4_trace1.rs:12, assertion `left == right` failed
Panicked at src/bin/ch4_unmap.rs:18, assertion `left == right` failed
Panicked at src/bin/ch4_unmap2.rs:18, assertion `left == right` failed
Panicked at src/bin/ch4_mmap3.rs:18, assertion `left == right` failed
make[1]: *** [run] Error 124
```

根因有两条：

1. **用户指针没有经过当前任务页表翻译。** 内核直接解引用用户地址既不可靠，也会在复制
   `TimeVal` 时丢掉页内偏移，导致用户结构体看到的时间为零。
2. **mmap/unmap/trace 仍是桩。** 页表已经存在，但系统调用没有把虚拟范围映射到物理帧，
   也没有在解除映射时回收 `FrameTracker`。

## 3. 实现方案

### 3.1 用户内存访问

在 `os/src/mm/page_table.rs` 增加统一的页表访问路径：

- `in_user_range` 先检查地址不回绕且位于 SV39 用户半空间；
- `fetch_frame` 拒绝无效叶 PTE；
- `read_user_byte` / `write_user_byte` 保留 `VirtAddr::page_offset()`；
- `write_user_bytes` 按页分块复制，支持 `TimeVal` 跨页。

`sys_get_time` 把秒/微秒编码为两个 `usize` 的字节表示，再通过当前任务的 satp 写回用户缓冲区。
`sys_trace` 的 Read/Write 也走同一组 helper，未映射、越界或权限不足时返回 `-1`，不让内核缺页。

### 3.2 mmap/unmap

`sys_mmap` 将 Prot 位转换为 `MapPermission`，拒绝空权限、未知位以及没有 R 位的 W/X 组合；
长度按页向上取整，起始地址必须页对齐。`TaskManager` 在独占访问内检查目标范围：

- mmap 要求整个范围当前无有效映射，随后插入匿名 `Framed` 区域；
- munmap 要求整个范围当前均有映射，且不能横跨一个不完全属于请求范围的 `MapArea`；
- 删除区域时逐页清 PTE，区域析构时 `FrameTracker` 自动归还物理帧。

`PageTable::is_mapped` 只把有效叶 PTE 视为映射，避免 munmap 后残留的空 PTE 被误判为已占用。

### 3.3 syscall 计数

计数仍放在 trap handler 的 syscall dispatch **之前**，所以 `sys_trace(Syscall, id, 0)` 查询自身时
能看到本次调用。ch4 使用每任务计数器数组，越界 id 返回 `None`，对应用户态 `-1`。

## 4. 编译与判分证据

修复接管时的四个编译错误后，`rig_c2_check.sh ch4` 结果：

```text
ch=ch4 check_rc=0
error_lines=0
warning_lines=0
```

完整判分 `oscamp2/logs/c_ch4_takeover2.log:313-330` 的 16 条判决：

```text
[PASS] found <get_time OK18890! (\d+)>
[PASS] found <Test sleep OK18890!>
[PASS] found <current time_msec = (\d+)>
[PASS] found <time_msec = (\d+) after sleeping (\d+) ticks, delta = (\d+)ms!>
[PASS] found <Test sleep1 passed18890!>
[PASS] found <string from task trace test>
[PASS] found <Test trace OK18890!>
[PASS] found <Test 04_1 OK18890!>
[PASS] found <Test 04_4 test OK18890!>
[PASS] found <Test 04_5 ummap OK18890!>
[PASS] found <Test 04_6 ummap2 OK18890!>
[PASS] found <Test trace_1 OK18890!>
[PASS] not found <FAIL: T.T>
[PASS] not found <Test sbrk failed!>
[PASS] not found <Should cause error, Test 04_2 fail!>
[PASS] not found <Should cause error, Test 04_3 fail!>
Test passed18890: 16/16
```

同轮日志 `:333` 首次触发报告闸：

```text
Report for lab1 needed. Add your report to reports/lab1.pdf or reports/lab1.md
MAKE_RC=2
```

补齐报告后的最终复跑日志为 `oscamp2/logs/c_ch4_report1.log:330-335`：

```text
Test passed1144: 16/16
Report for lab1 found.
Report for lab2 found.
MAKE_RC=0
```

至此代码判决、报告存在性与 Makefile 退出码三项同时满足。

## 5. 本机无法复现的步骤

- 课程 CI 的 QEMU 8.2.x 与 Ubuntu x86_64 宿主只能在 GitHub Actions 上验证；本机是 macOS arm64
  加 QEMU 11.1.3。
- 本机没有完整的 `rust-objcopy` / `readelf` 工具链，因此不提供 ELF 段表或反汇编输出。
- `user_lib` 编译日志里的 warning 来自固定测试代码，不是学生内核；内核侧以
  `rig_c2_check.sh` 的 `warning_lines=0` 为准。
