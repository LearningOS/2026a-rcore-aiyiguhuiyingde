# lab1 · rCore 第三章：二进制工具 / 内核构建与链接

| 项目 | 值 |
| --- | --- |
| 分支 | `ch8`（继承第四章分支已完成的 lab1 内容） |
| 账号 | `aiyiguhuiyingde` |
| 日期 | 2026-11-25 |
| 本报告路径 | `reports/lab1.md` |
| 判分结果 | ch3 代码 **7/7**（第 10 轮），报告闸门此时仍未满足 → `MAKE_RC=2` |

> 写作约束：本报告只描述**已经真实发生、且能被现存文件复核**的内容。本机没有可用的
> `rust-objcopy` / `rust-objdump` / `readelf`，也没有 riscv 交叉链接器和 qemu-user，
> 因此全文**不含任何 ELF 段表、符号表、反汇编输出或工具截图**。凡是我没能亲手复现的步骤，
> 都写在第 6 节「本机无法复现的步骤」里，并给出替代验证方式。每条证据后面标注了来源文件与行号，
> 可以逐条回查。

---

## 1. 实验目标与本章环境

### 1.1 目标

第三章把第二章的「批处理串行执行」升级为「多道程序 + 分时多任务」：所有 app 在启动时一次性
加载进内存，各自拥有一段固定的加载窗口和一套独立的内核栈/用户栈，靠时钟中断轮转。
本章新增一条内省通道 `sys_trace`，用于读取任意任务的 syscall 调用次数、读写指定地址上的字节。

lab1 这条线关注的是**镜像是怎么拼出来的**：链接脚本、`build.rs` 生成的 `link_app.S`、
固定加载窗口地址，以及 `rust-objcopy` 这类二进制工具在构建链路里的位置。

### 1.2 版本与环境（全部取自判分日志首部，非记忆）

判分脚本每轮都在日志开头打印实际使用的工具链版本，见
`/Users/eieieiei/Documents/Qoder/2026-11-20/91507e43/oscamp2/logs/c_ch3_trace10.log:2`：

```text
### rustc=rustc 1.80.0-nightly (c987ad527 2024-05-01) cargo=cargo 1.80.0-nightly (6087566b3 2024-04-30) qemu=QEMU emulator version 11.1.3 timeout=/usr/bin/timeout
```

同一行还出现在 `c_ch3_base.log:2` 以及 trace1…trace9 各轮的 `:2`，说明 11 轮用的是同一套工具链。

| 组件 | 仓库声明 | 本机判分日志实测 |
| --- | --- | --- |
| Rust 工具链 | `nightly-2024-05-02`（`rust-toolchain.toml:3`） | `rustc 1.80.0-nightly (c987ad527 2024-05-01)` |
| 目标平台 | `riscv64gc-unknown-none-elf`（`rust-toolchain.toml:5`；`os/Makefile:2`） | 日志 `:8` 行 `riscv64gc-unknown-none-elf (installed)` |
| `cargo-binutils` | `0.4.0`（`os/Makefile:38`） | 未真正安装，见下 |
| QEMU | README 表格写 `8.2.x`（评测用 8.2.2） | **`QEMU emulator version 11.1.3`** |
| RustSBI 固件 | `0.4.1`（`bootloader/rustsbi-qemu.toml:6`） | 串口 banner `RustSBI version 0.4.1`（`c_ch3_trace10.log:109`） |

两点必须如实记录：

1. **README 的 QEMU 版本与本机不一致。** README 承诺 Docker/GitHub 评测使用 QEMU 8.2.2，
   而本地判分副本跑的是 Homebrew 的 11.1.3。本章用到的机器特性只有 S-mode 异常/中断、
   `fence.i` 和 CLINT 定时器，日志显示行为与预期一致，但这是一个已知的环境偏差，不是我的实现问题。
2. **`rustup` / `cargo install` / `rust-objcopy` 在本机由 shim 顶替。** 判分日志
   `c_ch3_trace10.log:10,12,14`：

   ```text
   + shim cargo install (skipped)
   + shim rustup component add rust-src (skipped)
   + shim rustup component add llvm-tools-preview (skipped)
   ```

   这三行来自 `PATH` 前置的 shim 目录（判分脚本 `rig_c2_run.sh:36` 把变量 `$OSC` 下的 `shim/`
   放在 PATH 最前面；该目录不在本工作区内，本报告只引用脚本里的这一行，不打开它）。
   也就是说 `os/Makefile:38` 那条 `cargo install cargo-binutils --version 0.4.0 --locked`
   在本地判分时**什么都没装**，`llvm-tools-preview` 组件也**没有被添加**。
   后果直接体现在构建命令上：判分器会用 `overwrite/Makefile-ch3` 覆盖 `os/Makefile`
   （`ci-user/overwrite.py` 中 `chapter <= 5` 分支），替换后的 `run` 目标直接用
   `-kernel $(KERNEL_ELF)` 启动 ELF（`ci-user/overwrite/Makefile-ch3:17-22`），
   不再走仓库版 `os/Makefile:44` 的 `$(OBJCOPY) ... -O binary`。最终一轮日志里确实只有：

   ```text
   make -C ../os run OFFLINE= | tee stdout-ch3      # c_ch3_trace10.log:81
   cargo build --release                            # c_ch3_trace10.log:82
   timeout --foreground 30s qemu-system-riscv64 \
           -machine virt -nographic \
           -bios ../bootloader/rustsbi-qemu.bin \
           -kernel target/riscv64gc-unknown-none-elf/release/os   # c_ch3_trace10.log:103-107
   ```

   整份日志里搜不到任何 `rust-objcopy` / `.bin` 生成行。**结论：本机的判分链路根本没有执行过
   objcopy，`.bin` 镜像这一步在本地从未被真正验证过，只在 GitHub Actions 的 Docker 里才会跑到。**
   这也是本报告不能给出任何二进制工具输出的根本原因之一。

### 1.3 判分基线指纹

11 轮日志的 `:3` 行完全相同，checker 与测试集是被 pin 死的两个 commit：

```text
### checker=7d61ec55b58eed6ca7052917846c1b87af34563a tests=a0593662ad55d670ba8c27ce1763347cd0dd552f
```

（`c_ch3_trace10.log:3`，同样出现在 base 与 trace1…trace9。）
这条指纹很重要：**它排除了「换了测试所以从 5/7 变 7/7」这种解释**——十轮之间变化只可能来自我改的内核代码。

---

## 2. 内核镜像是怎么拼出来的

这一节全部基于源码阅读，没有运行任何构建命令。

### 2.1 链接脚本决定段的排布

`os/src/linker.ld`：

| 行号 | 内容 | 作用 |
| --- | --- | --- |
| `:3` | `BASE_ADDRESS = 0x80200000;` | 内核基址，与 RustSBI jump 模式投递地址一致 |
| `:11-14` | `.text : { *(.text.entry) *(.text .text.*) }` | `.text.entry` 排在最前，保证 `_start` 是第一条指令 |
| `:16-22` | `. = ALIGN(4K); .rodata` | rodata 4K 对齐后接在 text 之后 |
| `:24-30` | `. = ALIGN(4K); .data` | app 的二进制数据就嵌在这里 |
| `:32-39` | `.bss : { *(.bss.stack) sbss; ... }` | 引导栈 `boot_stack` 放 bss 最前面 |
| `:45-47` | `/DISCARD/ { *(.eh_frame) }` | no_std 下丢弃 unwind 信息 |

内核入口链条：`entry.asm` 的 `_start`（`os/src/entry.asm:3-5`：`la sp, boot_stack_top` → `call rust_main`）
→ `main.rs` 的 `rust_main()`。`main.rs:44-45` 两行 `global_asm!` 把 `entry.asm` 与生成的 `link_app.S`
一起编进内核。

RustSBI 侧的印证（串口原文，`c_ch3_trace10.log:143`）：

```text
[RustSBI] INFO  - Redirecting hart 0 to 0x00000080200000 in Supervisor mode.
```

以及 `bootloader/rustsbi-qemu.toml` 里 `mode = "jump"`、`kernel_address = 0x80200000`、
`firmware_address = 0x80000000`，与 linker.ld 的基址互相咬合。

### 2.2 `build.rs` 生成 `link_app.S`：把 app 当数据嵌进内核

`os/build.rs` 关键行：

- `:12` `static TARGET_PATH: &str = "../user/build/bin/";` —— 输入是 user 侧产出的 `.bin` 裸镜像；
- `:16` `File::create("src/link_app.S")` —— 每次编译重新生成这个汇编文件；
- `:26` `apps.sort()` —— **按文件名字典序排序**，这决定了 app_id 的顺序；
- `:28-42` 先写 `_num_app`（`.quad apps.len()`），再紧跟 `apps.len()` 个 `.quad app_{i}_start`，
  最后补一个 `.quad app_{len-1}_end`；
- `:44-56` 每个 app 一段 `.section .data` + `.incbin "{TARGET_PATH}{app}.bin"`，
  前后打上 `app_{i}_start` / `app_{i}_end` 符号。

于是内核 `.data` 里存在一张「数量 + 起始地址表 + 尾部哨兵」的表，`loader.rs` 用
`extern "C" { fn _num_app(); }` 把符号地址当作指针 `read_volatile` 读出来（`os/src/loader.rs:87-92`）。
`load_apps()` 取 `num_app_ptr.add(1)` 起、长度 `num_app + 1` 的切片作为地址表
（`loader.rs:102`），这正是上面那个哨兵存在的理由：最后一个 app 的结束地址要靠
`app_start[i+1] - app_start[i]` 算出来（`loader.rs:111`）。

值得注意的现实情况：本工作区 `os/src/link_app.S` 当前内容是

```asm
_num_app:
    .quad 0
    .quad app_18446744073709551615_end
```

（`os/src/link_app.S:6-7`，共 8 行）——`_num_app = 0`，且那个 `18446744073709551615` 就是
`0 - 1` 无符号回绕的结果，对应 `build.rs:42` 在 `apps.len() == 0` 时的输出。
原因是本工作区的 `user/build/bin/` 是空目录（`user/build/bin` 存在但里面没有任何文件），
判分时用的 app 来自判分副本 `c_run/ch3/ci-user/user/`，而那一轮的产物随后被
`make restore` / `rm -rf` 清掉了。**所以「13 个 app 被嵌进内核」这件事只能从判分日志的
`[build.py]` 输出确认，不能从工作区文件确认**，这一点在第 6 节还会重申。

### 2.3 固定加载窗口：数值全部抄自 `config.rs`

`os/src/config.rs`：

| 常量 | 值 | 行号 |
| --- | --- | --- |
| `USER_STACK_SIZE` | `4096` | `:6` |
| `MAX_APP_NUM` | `16` | `:12` |
| `APP_BASE_ADDRESS` | `0x80400000` | `:14` |
| `APP_SIZE_LIMIT` | `0x20000` | `:16` |
| `MEMORY_END` | `0x88000000` | `:21` |
| `SYSCALL_COUNT_SLOTS` | `512` | `:28` |
| `KERNEL_STACK_SIZE` | `4096 * 2` | `:8` |
| `KERNEL_HEAP_SIZE` | `0x20000` | `:10` |

窗口公式在 `os/src/loader.rs:57-59`：

```rust
fn get_base_i(app_id: usize) -> usize {
    APP_BASE_ADDRESS + app_id * APP_SIZE_LIMIT
}
```

即 app i 独占 `[0x80400000 + i*0x20000, 0x80400000 + (i+1)*0x20000)`，每格 128 KiB。
`load_apps()`（`loader.rs:96-125`）先把整个窗口清零，再把 `.data` 里的原始 ELF 拷进去，
最后执行一次 `fence.i`（`loader.rs:122-124`）——注释里写明这是 RISC-V Zifencei 的要求：
写完取指存储后必须 fence，后续取指才保证看到新代码。

`init_app_cx()`（`loader.rs:128-133`）用 `get_base_i(app_id)` 当入口、
`USER_STACK[app_id].get_sp()` 当初始 `sp`，构造 `TrapContext::app_init_context(entry, sp)`
压进该任务的**内核栈**顶部；`trap/context.rs:19-31` 里 `sstatus.SPP = User`，
这样 `sret` 出去就是 U-mode。

判分日志独立地证实了这套布局与 user 侧链接地址一致
（`c_ch3_trace10.log:68` 起，`[build.py]` 逐行打印每个 app 的加载地址）：

```text
[build.py] application ch2b_bad_address start with address 0x80400000
[build.py] application ch2b_bad_instructions start with address 0x80420000
[build.py] application ch2b_bad_register start with address 0x80440000
[build.py] application ch2b_hello_world start with address 0x80460000
[build.py] application ch2b_power_3 start with address 0x80480000
[build.py] application ch2b_power_5 start with address 0x804a0000
[build.py] application ch2b_power_7 start with address 0x804c0000
[build.py] application ch3_sleep start with address 0x804e0000
[build.py] application ch3_sleep1 start with address 0x80500000
[build.py] application ch3_trace start with address 0x80520000
[build.py] application ch3b_yield0 start with address 0x80540000
[build.py] application ch3b_yield1 start with address 0x80560000
[build.py] application ch3b_yield2 start with address 0x80580000
```

（`:68-80`，13 行；`ch3_trace` 在 `:77`。）
相邻地址差恰好是 `0x20000`，与 `APP_SIZE_LIMIT` 吻合；起点 `0x80400000` 与 `APP_BASE_ADDRESS` 吻合；
顺序与 `build.rs:26` 的字典序排序吻合。这是本章「构建/链接」部分我最满意的一条交叉验证：
**我没有 objdump，但 user 侧链接器的自述输出和内核侧的窗口公式对上了。**

顺带一条同源证据：`c_ch3_trace10.log:145` 的

```text
[kernel] PageFault in application, bad addr = 0x0, bad instruction = 0x804003a4, kernel killed it.
```

`bad instruction = 0x804003a4` 落在第一个窗口 `[0x80400000, 0x80420000)` 内，正是
`ch2b_bad_address` 的取指地址；说明窗口划分不只是纸面公式，运行时确实在那里执行。

### 2.4 静态容量核对（算术，不依赖工具）

`MAX_APP_NUM = 16`、`APP_BASE_ADDRESS = 0x80400000`、`APP_SIZE_LIMIT = 0x20000`：
最后一个窗口的上界是 `0x80400000 + 16 * 0x20000 = 0x80600000`，远小于 `MEMORY_END = 0x88000000`，
也不与内核镜像（基址 `0x80200000`）重叠。本章实际只用了 13 格（`0x80400000..0x805a0000`），
还剩 3 格余量。栈槽方面：`KERNEL_STACK` 数组 16 × 8 KiB = 128 KiB、`USER_STACK` 数组 16 × 4 KiB = 64 KiB
（`loader.rs:24-30`），都是 `.bss` 里的静态对象，不需要堆。

---

## 3. 本章实现：`sys_trace` 通道

改动清单（`git status --porcelain` 只读查询，7 个文件）：

```text
M os/src/config.rs
M os/src/loader.rs
M os/src/syscall/mod.rs
M os/src/syscall/process.rs
M os/src/task/mod.rs
M os/src/task/task.rs
M os/src/trap/mod.rs
```

`git diff --stat` 汇总为 `7 files changed, 221 insertions(+), 11 deletions(-)`。

### 3.1 三种 request 的语义

syscall 号在 `os/src/syscall/mod.rs:22` 定义：`SYSCALL_TRACE = 410`，分发在 `:37`
`SYSCALL_TRACE => sys_trace(args[0], args[1], args[2])`。

`TraceRequest` 是不带显式 repr 的普通 enum，判别式按声明顺序，因此
`Read = 0`、`Write = 1`、`Syscall = 2`（`os/src/syscall/process.rs:62-64` 把这三个常量写死并注明理由）。

| request | `id` 的含义 | `data` 的含义 | 返回值 |
| --- | --- | --- | --- |
| `Read`(0) | 待读字节地址 | 忽略 | 成功返回该字节；地址非法返回 `-1` |
| `Write`(1) | 待写字节地址 | 写入的字节 | 成功 `0`；地址非法 `-1` |
| `Syscall`(2) | 被统计的 syscall id | 忽略 | 当前任务调用该 syscall 的次数；id 越界 `-1` |
| 其它 | — | — | `-1`（`process.rs:133` 的兜底分支） |

三个动作都只作用于**当前任务**：ch3 没有进程句柄，无法指名另一个任务，
这也让下面那两条地址校验有意义（`process.rs:74-76` 的注释记录了这一取舍）。

两个容易踩的实现细节，我在源码里留了注释：

- `Read` 的返回值写成 `byte as u32 as isize`（`process.rs:107`）。若直接 `byte as isize`，
  Rust 会把 `u8` 零扩展到 `isize`，看似没问题；但如果中间经过任何有符号类型就会变成符号扩展，
  于是 `var >= 0x80` 时返回负数，user_lib 会把它当成 `-1` → `None`，
  正好撞上第 3.3 节的失败形态。零扩展 + 只用 `-1` 表示错误，是唯一安全的组合。
- `Read`/`Write` 都用 `read_volatile` / `write_volatile`（`process.rs:95`、`process.rs:113`）。
  测试程序在 `trace_write` 之后立刻用 `core::ptr::read_volatile(&var)` 比对
  （`ch3_trace.rs:43`），若内核侧的写被优化掉或推迟到 trap 返回之后，比较就会失败。

### 3.2 坑一：计数重复 / 漏计 —— 为什么计数放在 trap 入口

官方测试程序的断言（`oscamp2/c_run/ch3/ci-user/user/src/bin/ch3_trace.rs`）：

```rust
assert!(3 <= count_syscall(SYSCALL_GETTIMEOFDAY));   // :22
// 注意这次 sys_trace 调用本身也计入
assert_eq!(2, count_syscall(SYSCALL_TRACE));         // :24
...
assert_eq!(7, count_syscall(SYSCALL_TRACE));         // :34
```

`:24` 期望 2：此前调过一次 `count_syscall`，加上正在进行的这一次，共 2 次。
`:34` 期望 7：`2 + 1 println + 4 count_syscall`。也就是说 **trace 请求要计入自己**。

判分日志给出了三轮不同的错法，正好构成一组对照：

| 轮次 | 日志行号 | 串口 panic | 当时的错因 |
| --- | --- | --- | --- |
| base | `c_ch3_base.log:233` | `Panicked at src/bin/ch3_trace.rs:22, assertion failed: 3 <= count_syscall(SYSCALL_GETTIMEOFDAY)` | 直方图根本没记全：计数器只在个别 handler 里自增，`get_time` 没被记到 3 次 |
| trace1 / trace2 / trace7 / trace8 | `c_ch3_trace1.log:222` 等 | `Panicked at ...ch3_trace.rs:24` + `left: 2 / right: 1` | **少计 1**：`sys_trace` 分支里做了「本次不计入」的特判，读回来的数字把自己减掉了 |
| trace6 | `c_ch3_trace6.log:226` | `Panicked at ...ch3_trace.rs:24` + `left: 2 / right: 0` | **完全没计**：当时用一个白名单函数 `should_count_syscall()` 过滤「哪些 syscall 该计数」，`SYSCALL_TRACE` 被整个排除在外 |

三个错法的共同根源是同一个：**把「记不记、记几次」的判断分散到了各个 `sys_*` 处理函数里**。
分散写法必然出错——要么漏一个新 syscall，要么给某个 syscall 加了不该有的特判。

最终写法（`os/src/trap/mod.rs:53-64`）把记账移到 trap 分发器：

```rust
Trap::Exception(Exception::UserEnvCall) => {
    cx.sepc += 4;                 // :55  跳过 ecall
    record_syscall(cx.x[17]);     // :62  分发之前自增
    cx.x[10] = syscall(cx.x[17], [cx.x[10], cx.x[11], cx.x[12]]) as usize;  // :64
}
```

（`sepc += 4` 是为了让 `sret` 之后从 `ecall` 的下一条指令继续执行。）

选这里的理由写在 `trap/mod.rs:56-61` 的注释里，归纳成三点：

1. **唯一咽喉点**：所有用户 syscall 必经 `__alltraps` → `trap_handler`，不可能漏，
   也不需要任何 syscall 知道自己正在被统计；
2. **天然自计数**：自增发生在分发**之前**，正在执行的 `sys_trace` 已经被写进直方图，
   于是「trace 计入自己」不需要特判，只要读侧老老实实返回当前值即可；
3. **不会重复**：一次 `ecall` 只进一次 trap 入口，所以每个 syscall 恰好加 1。
   反过来，如果既在 trap 入口加、又在某个 `sys_*` 里加，就会出现 `right: 3` 这类超计数——
   本轮日志里没有留下这种形态，说明我最终没有走到重复计数那一步，中途的两次修正方向是对的。

直方图的存放位置也是一个决策点：`record_syscall` 落到
`TASK_MANAGER.record_current_syscall()`（`task/mod.rs:124-129`），
再落到 TCB 内的定长数组 `syscall_counts: [isize; SYSCALL_COUNT_SLOTS]`（`task/task.rs:21`）。
放在 TCB 里而不是侧表，是为了让计数在任务 `exit` 之后依然可读——
测试的 `assert_eq!(0, count_syscall(SYSCALL_EXIT))`（`ch3_trace.rs:27`、`:37`）
问的正是「我还没退出的时候 exit 计数是多少」，而 ch3 在启动时就为 16 个任务预建好 TCB
（`task/mod.rs:59-62`），所以未跑过的任务诚实报 0。
数组越界的 id 会被静默忽略而不是 panic（`task/task.rs:36-40`）：
一个非法 id 不应该把 tracer 自己打死。槽位大小 `512` 的依据写在 `config.rs:23-28`：
覆盖本章最大的 `SYSCALL_TRACE = 410` 与后续章节的 `SYSCALL_THREAD_CREATE = 460`，留出余量，
从而永远不需要堆或间接查表。

### 3.3 坑二：栈地址被校验拒掉

第 9 轮（6/7）的串口输出（`c_ch3_trace9.log:230-231`）：

```text
Panicked at src/bin/ch3_trace.rs:41, assertion `left == right` failed
  left: Some(111)
 right: None
```

对应测试源码 `let mut var = 111u8; assert_eq!(Some(111), trace_read(&var as *const u8));`
（`ch3_trace.rs:40-41`）。

排查过程值得记下来，因为方向一开始搞反了：

- `left: Some(111)` 是**期望值**（左边是 `Some(111)` 字面量），`right: None` 才是内核实际给出的结果。
  也就是 `trace_read` 拿到了 `-1` → user_lib 映射成 `None`。
- 所以失败的不是「读到的值不对」，而是**地址被拒绝了**。
- `var` 是一个局部变量，地址在**用户栈**上，不在 app 的代码/数据段里。
  而我最初版本的校验只认加载窗口 `[base_i, base_i + APP_SIZE_LIMIT)`，
  于是栈上一个合法的本任务变量被判成非法地址，返回 `-1`。

修复就是把「当前任务合法拥有的内存」明确定义为两块静态区域，
`os/src/loader.rs:73-84`：

```rust
pub fn is_valid_app_addr(app_id: usize, addr: usize, len: usize) -> bool {
    if len == 0 || app_id >= get_num_app() { return false; }
    let Some(end) = addr.checked_add(len) else { return false; };
    let base = get_base_i(app_id);
    let (stack_bottom, stack_top) = USER_STACK[app_id].bounds();
    (base <= addr && end <= base + APP_SIZE_LIMIT)
        || (stack_bottom <= addr && end <= stack_top)
}
```

配套的是 `UserStack::bounds()`（`loader.rs:49-53`）返回槽位的 `[bottom, top)`。
判定用 `checked_add` 防加法回绕，并要求**整个区间**都落在同一个区域内（跨区域的 `len` 一律拒绝）。

为什么必须覆盖「加载窗口 + 用户栈槽」这两块，而不是一块：

1. **正确性**：`init_app_cx()` 把 `sp` 指向 `USER_STACK[app_id]` 顶端（`loader.rs:131`），
   局部变量天然长在栈上；只校验代码窗口就会把自家内存拒了，就是 6/7 那一轮的现象。
2. **安全性**：反过来也不能放宽成「随便哪个窗口都行」。校验的对象是
   `current_task_id()`（`process.rs:92`、`:110`）——注意参数是「我是不是这个 app」而不是
   「你是不是某个 app」（`loader.rs:70-72` 注释记录了这点）。
   一旦允许跨任务地址，`sys_trace` 就成了任意物理读写原语：可以读内核镜像、
   读写邻居任务的数据段和邻居栈。这个校验正是 ch3 版的边界。
3. **可迁移性**：ch3 没有页表，所以「当前地址空间」恰好等价于这两块静态区域；
   到 ch4 有了页表，同一条查询会退化成「这个地址在当前地址空间里映射了吗」
   （对应上游 `translated_byte_buffer` 的位置）。把校验写成 `is_valid_app_addr(app_id, addr, len)`
   这一个函数，ch4 只需要换实现、不用换调用点。

第 10 轮同一段代码的输出（`c_ch3_trace10.log:227-229`）：

```text
string from task trace test

Test trace OK10959!
```

`Some(111)` 断言、`trace_write(main as *const _)`（`ch3_trace.rs:45`，代码窗口内的地址）、
以及最后的 `Test trace OK!` 全部通过。

### 3.4 顺带踩到的两处 lint 墙（0/7 三轮）

trace3/4/5 三轮连内核都没编出来，`Test passed: 0/7`，全部是本仓 `main.rs` 的两条严格 lint 拦的：

| 轮次 | 报错 | 日志行号 | 修法 |
| --- | --- | --- | --- |
| trace3 | `error[E0603]: function import 'should_count_syscall' is private` + `error: unused import: 'SYSCALL_GET_TIME'` | `c_ch3_trace3.log:97,114` | 删掉白名单方案 |
| trace4 | 同上 `E0603`（1 error） | `c_ch3_trace4.log:97` | 同上，改为 trap 入口统一计数 |
| trace5 | `error: missing documentation for a struct` / `for a struct field` ×2 | `c_ch3_trace5.log:97,109,115` | 给 `TimeVal` 及其字段补 doc |

根因在 `os/src/main.rs:18-19` 的 `#![deny(missing_docs)]` 与 `#![deny(warnings)]`：
`unused_imports` 被 `deny(warnings)` 升级成硬错误，公开项缺 doc 也是硬错误。
这算是本章构建链路的隐性门槛，写报告时值得提一句，否则后来人会在同样的地方卡三轮。

---

## 4. 验证方法与结果

### 4.1 十轮的真实进度

判分入口是 `oscamp2/rig_c2_run.sh`（只读其逻辑，未在本报告期间执行）：
它把工作区拷成一次性副本 `oscamp2/c_run/ch3/`、克隆好的 checker 放进去、
`export PATH="$OSC/shim:$NIGHTLY/bin:/opt/homebrew/bin:..."`，然后
`cd ci-user && make test CHAPTER=$N`，整段输出重定向到 `oscamp2/logs/c_${CH}_${SUF}.log`，
末尾追加 `MAKE_RC=$?`（`rig_c2_run.sh:42-48`）。

| 日志 | 绝对路径前缀 `/Users/eieieiei/Documents/Qoder/2026-11-20/91507e43/oscamp2/logs/` | 结果 | 卡在哪 |
| --- | --- | --- | --- |
| `c_ch3_base.log` | 基线 | 5/7 | `ch3_trace.rs:22` 计数不全 |
| `c_ch3_trace1.log` | | 5/7 | `:24` `2 vs 1` |
| `c_ch3_trace2.log` | | 5/7 | `:24` `2 vs 1` |
| `c_ch3_trace3.log` | | 0/7 | 编译失败 E0603 |
| `c_ch3_trace4.log` | | 0/7 | 编译失败 E0603 |
| `c_ch3_trace5.log` | | 0/7 | 编译失败 missing_docs |
| `c_ch3_trace6.log` | | 5/7 | `:24` `2 vs 0` |
| `c_ch3_trace7.log` | | 5/7 | `:24` `2 vs 1` |
| `c_ch3_trace8.log` | | 5/7 | `:24` `2 vs 1` |
| `c_ch3_trace9.log` | | 6/7 | `:41` `Some(111) vs None` 栈地址被拒 |
| `c_ch3_trace10.log` | | **7/7** | 代码全绿，只剩报告闸门 |

`Test passed` 行的行号在各轮不同（例如 trace10 在 `:245`、trace9 在 `:248`、base 在 `:249`），
上表的分数是从各轮日志 grep 出来的原文。

### 4.2 最终一轮的逐条 PASS 原文

`/Users/eieieiei/Documents/Qoder/2026-11-20/91507e43/oscamp2/logs/c_ch3_trace10.log`，`:237-245`：

```text
[PASS] found <get_time OK10959! (\d+)>
[PASS] found <Test sleep OK10959!>
[PASS] found <current time_msec = (\d+)>
[PASS] found <time_msec = (\d+) after sleeping (\d+) ticks, delta = (\d+)ms!>
[PASS] found <Test sleep1 passed10959!>
[PASS] found <string from task trace test>
[PASS] found <Test trace OK10959!>

Test passed10959: 7/7
```

七个正则来自 `oscamp2/c_run/ch3/ci-user/check/ch3.py:3-16` 的 `EXPECTED_3`
（`get_time OK`、`Test sleep OK`、`current time_msec`、`time_msec ... after sleeping`、
`Test sleep1 passed`、`string from task trace test`、`Test trace OK`），
匹配逻辑在 `check/base.py:12-17`，`count == total` 的断言在 `check/base.py:27`。

关于后缀数字：`OK10959` / `passed10959` 不是我写的字符串。判分器每轮用随机数改写测试与检查器
（`ci-user/Makefile:91-95` 的 `randomize` 目标，`RAND := $(shell awk 'BEGIN{srand();printf("%d", 65536*rand())}')`
在 `:1`），目的是防止有人把期望输出硬编码进内核。所以 `10959` 只是本轮随机 token，
它在 `c_ch3_trace10.log:15-18` 的四条 `sed` 命令行里出现，与 `:245` 的 `7/7` 相互印证。

同时，本轮串口里内核的收尾仍然正常（`c_ch3_trace10.log:231`）：

```text
[kernel] Panicked at src/task/mod.rs:159 All applications completed!
```

这条 panic 是 ch3 的预期终点（`task/mod.rs:158-160`：没有 Ready 任务时以 panic 收尾），
不是缺陷。对比基线轮，同一处当时在 `src/task/mod.rs:135`（`c_ch3_base.log:235`），
行号变化只是因为我在该文件里插入了新的记账函数——这一点也从侧面证明两轮之间改的是同一份代码。

### 4.3 `MAKE_RC` 为什么当时不是 0

`c_ch3_trace10.log` 结尾 `:246-250`：

```text
/Library/Developer/CommandLineTools/usr/bin/make restore
Restoring original files from temp-* dirs
Report for lab1 needed. Add your report to reports/lab1.pdf or reports/lab1.md
make: *** [test] Error 1
MAKE_RC=2
```

规则在 `oscamp2/c_run/ch3/ci-user/Makefile`：

- `:15-16` `ifeq ($(CHAPTER), 3) → LAB := 1`：第三章要求 1 份报告；
- `:10-12` 学生路径（不带 TA token）会置 `CHECK_REPORTS := 1`、`BASE := 2`；
  日志 `:20` 的 `make -C user build BASE=2 TEST=3 CHAPTER=3` 正说明走的是学生路径；
- `:130-139` 收尾循环：`for i in $(seq $(LAB))`，若 `../reports/lab$i.pdf` 和 `../reports/lab$i.md`
  都不存在就打印上面那句 `Report for lab1 needed.` 并 `exit 1`。

所以 `MAKE_RC=2` 的成因非常明确：**七项测试已经全绿，退出码只被报告闸门扣住**。
本报告（`reports/lab1.md`）就是为了补上这一格；写完后同一轮应当变成
`Report for lab1 found.` 且 `MAKE_RC=0`。我没有重跑判分（第 1 节边界里说明了原因：
机器上正在跑判分编译），这一条属于「预期效果」而非「已复现结果」，特此标明。

另外，`:246-247` 的 `make restore` 解释了为什么判分副本里看不到任何中间产物：
`ci-user/Makefile:64-89` 会把 `os/`、`user/`、`check/` 从 `temp-*` 备份还原再删除目录，
判分结束后 `c_run/ch3/` 只留下源码级拷贝（现在 `c_run/ch3/ci-user/user/build/{bin,elf}` 均为空，
`c_run/ch3/os/target` 不存在），唯一的持久证据就是 `logs/` 下的日志本身。

---

## 5. 我实际做过的验证手段（除判分日志之外）

| 手段 | 用途 | 出处 |
| --- | --- | --- |
| 读 `logs/*.log` 的 panic 行与 `left/right` 值 | 区分「少计」「没计」「地址被拒」三类失败 | 第 3.2、3.3 节的表 |
| 比对 11 轮 `:3` 的 checker/tests commit 指纹 | 排除「测试变了所以过了」 | 第 1.3 节 |
| 用 `[build.py]` 打印的加载地址反推窗口公式 | 无 objdump 情况下验证链接布局 | 第 2.3 节 |
| 用 `bad instruction = 0x804003a4` 定位窗口归属 | 运行时交叉验证 | 第 2.3 节末 |
| 手工算术核对 `0x80400000 + 16*0x20000 < MEMORY_END` | 确认静态容量不越界 | 第 2.4 节 |
| 读 `ci-user/Makefile` 与 `overwrite.py` | 弄清判分实际用的是哪份 Makefile、为何没有 objcopy | 第 1.2、4.3 节 |

---

## 6. 本机无法复现的步骤，以及替代做法（诚实清单）

| 想做的步骤 | 为什么做不了 | 本报告的替代做法 |
| --- | --- | --- |
| `rust-objcopy os.elf --strip-all -O binary os.bin` | 本机没有 `rust-objcopy`（`command -v` 查不到）；判分链路的 `cargo install cargo-binutils` 被 shim 跳过（日志 `:10`） | 引用仓库版 `os/Makefile:24,44` 说明命令形状；引用判分版 `overwrite/Makefile-ch3:17-22` 说明本地根本不走 objcopy（直接用 `-kernel ELF`） |
| `rust-objdump -d/-x` 看内核反汇编 | 同上，`rust-objdump` 不存在；`os/Makefile:54-60` 的 `disasm` 目标本地不可用 | 不写任何反汇编内容；只用 `[build.py]` 的地址行与 `bad instruction` 行做地址层面论证 |
| `readelf -S/-l` 看 ELF 段表/程序头 | 本机没有 `readelf`（macOS 自带 `objdump` 不认 riscv64 ELF） | 段排布只从 `os/src/linker.ld` 的文字定义叙述，不给任何地址数值（我没有可靠来源） |
| 打印 `stext/srodata/sdata/sbss` 等内核段地址 | `main.rs:59-93` 的 `kernel_log_info()` 用 `trace!/debug!/info!/warn!/error!` 输出，而 `logging.rs:37-44` 在未设 `LOG` 环境变量时把级别设为 `LevelFilter::Off`；判分日志里也确实只有 `[kernel] Hello, world!`（`:144`），没有任何段地址行 | 明确声明「本机拿不到段地址」，不编造数值 |
| 展示 `link_app.S` 嵌入 13 个 app 的真实内容 | 工作区 `user/build/bin/` 为空，`os/src/link_app.S` 现在是 `_num_app = 0` 的空壳；判分副本的产物又被 `make restore` 清掉 | 讲清 `build.rs` 的生成规则（第 2.2 节），并用 `c_ch3_trace10.log:68-80` 的 13 行 `[build.py]` 输出证明「判分那一轮确实是这 13 个 app」 |
| 自己 `make -C os run` 复跑一遍 | 任务边界禁止运行 `cargo`/`make`/`qemu`（机器上正在跑判分编译） | 完全依赖 `logs/` 里已有的真实串口输出，不新增运行 |
| 用 riscv qemu-user 单测某个 app | 本机没有 riscv 交叉链接器与 qemu-user | 只依据官方测试程序 `ch3_trace.rs` 的断言来推断内核行为 |
| 重跑判分确认 `MAKE_RC=0` | 同上禁令 + 判分正在占用 | 第 4.3 节把闸门条件写成可复核的规则引用（`ci-user/Makefile:15-16,130-139`），留给下一次判分自然验证 |

一句话总结这一节：**本实验的所有数值型证据都来自判分日志原文与本仓源码文本，
没有任何一条来自我没跑成功的二进制工具。**

---

## 7. 结论

1. **代码状态**：ch3 的 `sys_trace` 通道实现完成，最后一轮判分
   `Test passed10959: 7/7`（`oscamp2/logs/c_ch3_trace10.log:245`），
   七条正则全部 `[PASS]`（`:237-243`）。改动集中在 7 个文件、净增 221 行。
2. **两条核心经验**：
   - syscall 记账必须放在 trap 入口这个唯一咽喉点、且在分发之前，
     「trace 计入自己」才会自动成立；分散到各 handler 一定产生漏计或重复计
     （日志里 `2 vs 1`、`2 vs 0` 两种形态就是这两个错的现场）。
   - ch3 的地址校验必须同时覆盖「本任务的加载窗口」和「本任务的用户栈槽」，
     少一块就会把自家局部变量拒掉（`6/7` 那轮的 `Some(111) vs None`）；
     多一块（允许跨任务）就会把内省通道变成任意物理读写原语。
3. **报告状态**：本报告位于
   `/Users/eieieiei/Documents/Qoder/2026-11-20/91507e43/oscamp2/rcore_wt/ch3/reports/lab1.md`，
   文件名与 `ci-user/Makefile:132` 检查的 `../reports/lab1.md` 完全对应。
   上一轮的 `Report for lab1 needed.`（`c_ch3_trace10.log:248`）与 `MAKE_RC=2` 应由本文件解除。
4. **后续**：第四章需要 `lab1` + `lab2` 两份报告（`README.md:119` 的表格、
   `ci-user/Makefile:17-18` 的 `CHAPTER=4 → LAB := 2`）。本报告将在 ch4 分支沿用，
   届时只需补 `lab2`；同时 ch4 会把本章的 `is_valid_app_addr(app_id, ..)` 换成页表遍历，
   调用点保持不变——这也是第 3.3 节特意把校验收进单个函数的原因。
