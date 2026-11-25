# lab4 · rCore 第八章分支上的文件系统与描述符层

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch8`（文件系统机制继承自第六章分支，本分支未改动其逻辑，报告记录实际代码形态） |
| 主题 | `File` 抽象、easy-fs 之上的 `OSInode`、环形缓冲管道、文件描述符表 |
| 判分口径 | `EXPECTED_8` 由 `EXPECTED_6b` 叠加并发项而来，其中文件系统相关判决在本分支基线轮全部通过 |

## 1. 抽象层：一个 `File` trait 收四种实现

`os/src/fs/mod.rs` 只声明最小接口——`readable()`、`writable()`、`read(UserBuffer)`、
`write(UserBuffer)`，于是 stdin/stdout、磁盘 inode、管道在 `sys_read`/`sys_write` 里走同一条路径。
`fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>`（`task/process.rs:39`）按进程持有，
`alloc_fd()`（`:62`）优先复用 `None` 空洞、没有空洞才 `push(None)` 扩容，这使 `close` 之后
的 `open` 能拿回同一个小编号，测试里对 fd 数值的断言才不会漂。

`os/src/fs/stdio.rs:43` 的 `Stdout` 把「方向错误」直接做成可观察行为：`readable()` 返回 `false`、
`read()` 里 `panic!("Cannot read from stdout!")`。系统调用层先查 `readable()` 再调用 `read()`，
所以用户态误读得到的是 `-1` 而不是内核 panic。

## 2. `open_file` 的三种语义

`os/src/fs/inode.rs:99`：

- 带 `CREATE`：先 `ROOT_INODE.find(name)`，命中就 `inode.clear()` 后返回（等价于截断重写），
  未命中则 `ROOT_INODE.create(name)`，失败返回 `None` → 系统调用层给 `-1`；
- 不带 `CREATE` 但带 `TRUNC`：`find` 命中后 `clear()`；
- 普通只读打开：`find` 命中即包装为 `OSInode::new(readable, writable, inode)`。

`flags.read_write()` 把 POSIX 的 `O_RDONLY/WRONLY/RDWR` 拆成两个布尔，权限判断因此落在
包装对象上而不是磁盘 inode 上，`easy-fs` 本身仍只管目录项与数据块。
`OSInode::read_all()`（`:34`）是为 `sys_exec` 准备的整体读取接口，返回 `Vec<u8>`，
`sys_exec` 拿它喂 `process.exec()`（见 lab3）。

## 3. 管道：32 字节环形缓冲

`os/src/fs/pipe.rs`：`const RING_BUFFER_SIZE: usize = 32;`（`:34`），内部 `arr: [u8; 32]` 配
`head`/`tail`，`read`/`write` 各自按 `(x + 1) % RING_BUFFER_SIZE` 推进（`:67`、`:75`）；
可读字节数与可写字节数分别由 `available_read()`（`:87`）和 `available_write()`（`:94`）算出。
`make_pipe()`（`:103`）返回两个 `Arc<Pipe>`，它们**共享同一个** `PipeInner`，只有
`readable()`/`writable()`（`:113`、`:116`）不同——这就是 `pipetest passed!` 所需的最小实现：
读端写会得 `-1`，写端读会得 `-1`，两端句柄指向同一缓冲。

`sys_pipe`（`syscall/fs.rs:90`）分配两个 fd 并把两个编号经 `translated_refmut(token, pipe)` 与
`translated_refmut(token, pipe.add(1))` 写回用户数组——用户指针同样是先翻译再解引用。
`sys_dup`（`:108`）只 `Arc::clone` 表项，因此两个 fd 共享读写位置与缓冲，符合测试预期。

## 4. 用户缓冲区：`UserBuffer` 而不是裸指针

`sys_write`（`syscall/fs.rs:6`）与 `sys_read`（`:30`）都用
`UserBuffer::new(translated_byte_buffer(token, buf, len))` 构造缓冲，
`translated_byte_buffer` 在页表层把用户虚拟地址换成内核可见的物理字节切片，
因此跨页、未映射、权限不足三类情况在翻译阶段就被挡下。两个函数还刻意在拿到 `file.clone()`
之后 `drop(inner)` 再调用 `file.read/write`，避免 `write` 内部再取进程锁造成多重借用。

## 5. 本分支未实现的部分（如实记录）

`sys_fstat`（`syscall/fs.rs:127`）、`sys_linkat`（`:136`）、`sys_unlinkat`（`:145`）在 ch8 分支上
仍是模板桩（打印 `NOT IMPLEMENTED` 返回 `-1`）：`EXPECTED_8` 不含 hardlink、`fstat` 与 `unlink`
相关判决，本章判分对它们无要求，因此本分支保持原样，对应实现属于 `ch6` 分支的交付物。

验证入口：`bash oscamp2/rig_c2_run.sh ch8 <后缀>`（官方 `make test CHAPTER=8`，
checker/tests 固定到 `7d61ec55`/`a0593662`），文件系统项在基线轮 `logs/c_ch8_base2.log` 即为通过。
