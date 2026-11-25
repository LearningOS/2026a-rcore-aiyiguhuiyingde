# lab1 · 第六章（上）：磁盘上的文件系统与它上面的三层抽象

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch6`（`Initialize ch6` 之上的实现） |
| 主题 | easy-fs 的磁盘布局、块缓存、`OSInode`/`File` 抽象、`open`/`read`/`write`/`close` |
| 判分口径 | `EXPECTED_6` 中的 `Test file0 OK!`、`file_test passed!`；全套 27 个子测例不得回归 |
| 本轮读数 | `Test passed21749: 31/31`（`oscamp2/logs/c_ch6_final.log`） |

## 1. 磁盘镜像是谁生成的

仓库里的 `easy-fs-fuse/src/main.rs` 是给人手工打包用的：`main.rs:58` 把文件截到
`16 * 2048 * 512` 字节（**32768 块**），`main.rs:62` 调
`EasyFileSystem::create(block_file, 16 * 2048, 1)`，再逐个
`root_inode.create(name)` + `write_at(0, elf)`（`main.rs:79`、`:81`）。

**判分时跑的不是这份。** `rCore-Tutorial-Checker/Makefile:115` 先执行
`python3 overwrite.py 6`，`overwrite.py:16-17` 把 `overwrite/Makefile-ch6` 覆盖成
`os/Makefile`、把 `overwrite/easy-fs-fuse.rs` 覆盖成 `easy-fs-fuse/src/main.rs`。
覆盖后的 fuse 用的是另一套参数（`easy-fs-fuse.rs:58`、`:62`）：

```rust
f.set_len(14000 * 512).unwrap();
let efs = EasyFileSystem::create(block_file.clone(), 14000, 1);
```

日志 `oscamp2/logs/c_ch6_link1.log:175` 记下了实际那条命令——
`easy-fs-fuse -s ../ci-user/user/build/elf -o ../ci-user/user/build/fs.img`，
紧跟着 179-230 行是 `root_inode.ls()` 回打的 **52 个**目录项（`ch2b_*` 到 `ch6b_*` 全套 ELF），
即第一次挂载时根目录 `size = 52 * 32 = 1664` 字节、占 4 个数据块。
运行内核用的 Makefile 也是覆盖出来的那份：`overwrite/Makefile-ch6:12-15` 的 `fsimg`
目标 + `:23` 的 `timeout --foreground 30s qemu-system-riscv64` +
`:28-29` 的 virtio-blk 挂载；仓库里 `os/Makefile:75` 那个不带盘的 `run` 目标在判分时不会被用到。

所以真正要紧的是：**`EasyFileSystem::create` 是被两套调用共同使用的同一个函数**，
签名 `(block_device, total_blocks, inode_bitmap_blocks)` 不能动；
`DiskInode` 的格式改动只要内核侧与 `create` 侧同步就自洽——
这条前提决定了 lab4 里 `nlink` 可以上盘（详见该篇 §1）。

`create`（`easy-fs/src/efs.rs:24`）排出来的布局，两套调用各算一份：

| 区域 | 块号 | 判分路径（`total_blocks = 14000`） | 仓库 fuse（`32768`） |
| --- | --- | --- | --- |
| SuperBlock | 0 | 1 | 1 |
| inode bitmap | 1 | 1（由参数 `inode_bitmap_blocks` 给定） | 1 |
| inode area | 2 起 | 1366 | 1366 |
| data bitmap | 1368 起 | 4 | 8 |
| data area | 紧随 data bitmap | 1372 起，12628 块 | 1376 起，31392 块 |

inode area 那 1366 块与 `total_blocks` 无关：它的尺寸只由 inode bitmap 能报出的
`maximum()`（`bitmap.rs:67`：`blocks * BLOCK_BITS` = 4096）决定，见下一节。

SuperBlock 只是把这几个数字记下来（`layout.rs:48` 的 `initialize`），
`EasyFileSystem::open`（`efs.rs:89`）再据它把两个 `Bitmap` 的窗口复原，
`is_valid()` 靠 `EFS_MAGIC` 判断镜像是不是我们的格式。

## 2. `DiskInode` 变宽之后，inode area 的算法必须跟着改

原教程算 inode area 用的是字节数：

```rust
let inode_area_blocks =
    ((inode_num * core::mem::size_of::<DiskInode>() + BLOCK_SZ - 1) / BLOCK_SZ) as u32;
```

这个式子隐含一个假设：**`size_of::<DiskInode>()` 整除 `BLOCK_SZ`**。
因为 `get_disk_inode_pos`（`efs.rs:119`）是按 `BLOCK_SZ / inode_size` **向下取整**
算每块能塞几个 inode 的，块尾剩下的字节永远用不上。

本分支给 `DiskInode` 加了 `nlink`（`layout.rs:94`），实测尺寸从 128 变成 **132**：
每块只剩 3 个 inode，块尾浪费 44 字节。老式子会算出 1056 块、只够 3168 个 inode，
而 inode bitmap 的 `maximum()` 仍然报 4096——于是 id 从 3168 到 4095 的 inode
会被写到 data bitmap 的地盘上去。所以 `efs.rs:35-37` 改成按「个」而不是按「字节」取整：

```rust
let inodes_per_block = BLOCK_SZ / core::mem::size_of::<DiskInode>();
let inode_area_blocks = ((inode_num + inodes_per_block - 1) / inodes_per_block) as u32;
```

对 128 字节的旧布局，两个式子给出同一个 1024，**改动是纯修正而非行为变更**；
对 132 字节则给出 1366（容量 4098 ≥ 4096）。代价是 inode 区多出 342 块，
data area 相应少 342 块：判分路径的 14000 块镜像里从 12970 降到 12628（约 2.5%），
仓库那份 32768 块镜像里从 31734 降到 31392。两套调用都满足
`data_area_start_block + data_area_blocks == total_blocks`（1372+12628=14000、
1376+31392=32768），也就是说 §1 表里的分区没有互相重叠，也没有越出镜像末尾。

## 3. 从 `read(fd, buf)` 到磁盘要穿过几层

```
sys_read / sys_write            os/src/syscall/fs.rs
  └ UserBuffer（跨页缓冲切片）    os/src/mm/page_table.rs
      └ <dyn File>::read/write   os/src/fs/mod.rs:9
          └ OSInode              os/src/fs/inode.rs:20   ← 记 offset
              └ easy_fs::Inode   easy-fs/src/vfs.rs:10    ← 记 (block_id, block_offset, id)
                  └ DiskInode    easy-fs/src/layout.rs:83 ← 记 size/direct/indirect1/indirect2
                      └ BlockCache  easy-fs/src/block_cache.rs:18 ← 512 字节 LRU + dirty
                          └ BlockDevice（virtio-blk / 主机文件）
```

各层职责上有一条明确的界线，抄一遍容易，说清楚才有用：

1. **`OSInode` 只有「读到的位置」**。`OSInodeInner{offset, inode}`（`inode.rs:26`），
   每个打开的文件描述符持有一个 `Arc<OSInode>`，因此两个 fd 指向同一文件时
   各自的 offset 互不干扰（`read`/`write` 见 `inode.rs:156`、`:169`，
   按 `UserBuffer` 的分段逐段推进）。
2. **`easy_fs::Inode` 只有「怎么找到这个 inode」**。它不做分配决策，
   需要新数据块时向 `EasyFileSystem` 要（`vfs.rs:93` 的 `increase_size`），
   而且全程持着 `Arc<Mutex<EasyFileSystem>>`——`alloc_data()` 会改 bitmap，
   不锁不行。
3. **`DiskInode` 只有「size 与块号的映射」**。`read_at`/`write_at`
   （`layout.rs:320`、`:361`）把 `[offset, offset+len)` 按 512 字节切段，
   段内再用 `start % BLOCK_SZ` 定位；`get_block_id`（`:148`）负责 direct/indirect1/indirect2 三档。
   它假定块已经分配好了——`write_at` 的 `end` 会被 `self.size` 截断，
   所以扩 size 必须发生在写之前，这正是 `Inode::write_at`（`vfs.rs:286`）的调用顺序。
4. **`BlockCache` 是唯一碰设备的层**。`get_block_cache`（`block_cache.rs:130`）
   按块号取缓存项，`read`/`modify`（`:56`、`:60`）把块内存成 `&T`；
   `modify` 打脏标记，`block_cache_sync_all`（`:139`）在可能被外部观察到之前把脏块写回。

第 4 层解释了一个容易忽略的点：内核侧的 `Inode::write_at` 末尾会
`block_cache_sync_all()`（`vfs.rs:292`），而 `read_at` 不会——只读不需要落盘。
`link`/`unlink` 改的是「目录项 + inode 元数据」两处，所以我在两处都显式同步
（见 lab4）。

## 4. `open_file` 的 flags 语义

`OpenFlags`（`inode.rs:76`）沿用 `O_*` 的位值，`read_write()`（`:93`）把
`RDONLY`/`WRONLY`/`RDWR` 折成 `(readable, writable)`，空值按只读处理。
`open_file`（`:105`）的分支值得记住，因为它决定了 unlink/create 之间的相互作用：

| flags | 文件已存在 | 文件不存在 |
| --- | --- | --- |
| 含 `CREATE` | `inode.clear()` 截断，复用旧 inode | `ROOT_INODE.create(name)` 新建 |
| 含 `TRUNC`（无 `CREATE`） | `inode.clear()` 后打开 | 失败返回 `None` |
| 都不含 | 直接打开 | 失败返回 `None` |

`clear()`（`vfs.rs:296`）调 `DiskInode::clear_size` 收走全部数据块再逐个
`fs.dealloc_data()`，但它**不动 `nlink`、也不删目录项**——一个被硬链接着的文件
被 `open(..., CREATE)` 打开时，内容清空而所有名字仍然有效，这正是 POSIX 的语义。
顺带说明 `ch6_file3` 为什么能安全地反复 `CREATE` 同一个名字。

`sys_open` 本身（`syscall/fs.rs:51`）只做三件事：`translated_str` 取路径、
`open_file` 拿到 `Arc<OSInode>`、`inner.alloc_fd()`（`task/task.rs:89`）在
`fd_table` 里找空位或补一格。注意 `OpenFlags::from_bits(flags).unwrap()`
是骨架里就有的写法，未知位会 panic；测例传的位都在 `bitflags` 定义内，
本实验没有去改这条路径。

## 5. 判分契约与实测

| 测例 | 断言 | 依赖的本篇机制 |
| --- | --- | --- |
| `ch6b_filetest_simple` | 写 `"Hello, world!"` 再读回相等，打印 `file_test passed!` | `create` + `write_at` 扩块 + `read_at` 按块回读 |
| `ch6_file0` | `open(CREATE\|WRONLY) > 0`、写、`close`、再 `open(RDONLY)` 读回 `"Hello, world!"` | 两次 open 的独立 offset；`close` 只丢 `Arc` |

同一轮 `make test CHAPTER=6` 里，这两个测例分别打印
`file_test passed21749!`（日志第 541 行）与 `Test file0 OK21749!`（第 544 行），
`ch6_usertest` 的 27 个子测例全部跑完、无 `panic`、无 30 秒超时。
