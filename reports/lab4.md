# lab4 · 第六章（完）：硬链接——名字与 inode 分家之后要还的账

| 项目 | 内容 |
| --- | --- |
| 分支 | `ch6` |
| 主题 | `nlink` 的存放位置、`link`/`unlink` 的磁盘语义、`fstat` 的数据来源 |
| 判分口径 | `EXPECTED_6` 中的 `Test fstat OK!`、`Test link OK!`、`Test mass open/unlink OK!` |
| 代码位置 | `easy-fs/src/layout.rs:94/107`、`easy-fs/src/vfs.rs:167/201`、`easy-fs/src/efs.rs:137`、`os/src/fs/inode.rs:133/145/188`、`os/src/syscall/fs.rs:85/117/136` |

## 1. `nlink` 放在哪里，决定了后面所有事的写法

`ch6_file2` 的全部断言只围绕一个数字：

```
open("fname2", CREATE|WRONLY)      -> nlink == 1   (fstat 在 fd 上问的)
link("fname2", "linkname0")        -> nlink == 2
link ×2 更多                        -> nlink == 4
unlink("fname2")                   -> nlink == 3   (此时 fd 换成 open("linkname0"))
unlink ×2                           -> nlink == 1
```

一条硬链接就是目录里多出的一个 `DirEntry`，所以这个数字要么**存在 inode 上**，
要么**每次去目录里数**。本分支选前者：

```rust
// easy-fs/src/layout.rs:83-95
pub struct DiskInode {
    ...
    type_: DiskInodeType,
    /// How many directory entries name this inode. ...
    pub nlink: u32,
}
```

`initialize` 把它置 1（`layout.rs:107`）——`create` 追加的那条目录项就是被数着的那条，
不需要再 `+= 1`。代价是 `DiskInode` 从骨架的 128 字节变成 132 字节，
inode 区每块只能装 3 个（原来是 4 个），于是 inode area 的块数不能再按字节算。
这一条连同分区数字在 lab1 §2 已经算过，这里只强调它的因果：
**是「nlink 上盘」这个选择逼出了 `efs.rs:35-37` 的按个取整公式**，不是先改了布局再补测例。

另一个必须承认的后果：`nlink` 一旦上盘，`unlink` 就有义务维护它，
漏一次就是永久性的错账；而按扫描计数的写法不会累积错误，但每次 `fstat`
都要遍历整个目录，而且必须回答「我扫的是哪个目录」——单层根目录下成立，
两条路的取舍就在这儿。本分支的选择换来的是 `Inode::link_count()`
（`vfs.rs:40`）一次块缓存读取，以及下面第 2 节的顺手收益。

## 2. `Inode` 直接带 `id`：`ino` 断言免费拿到

```rust
// easy-fs/src/vfs.rs:10-16
pub struct Inode {
    id: u32,
    block_id: usize,
    block_offset: usize,
    ...
}
```

`Inode::new`（`:20`）多收一个 `id` 参数，`EasyFileSystem::root_inode`（`efs.rs:111`）传 0，
`create` 传 `alloc_inode` 刚归还的那个数。`ch6_file2` 里有

```rust
assert_eq!(stat2.ino, stat.ino);   // 两个名字必须报出同一个 inode
```

名字不同的两条目录项，指向的是同一个 inode——所以「同一个 inode」这件事
不需要从目录反查出来，构造句柄时就知道自己是谁。`stat()`（第 4 节）因此
只需 `inner.inode.inode_id()`。

## 3. `link`：两条拒绝，一次追加

`vfs.rs:167`。与 `create`（`:110`）相比，`link` 多了两次拒绝，
而且两次判断都必须在 `fs` 锁内做——否则另一个任务可以挤在「查空位」和「写目录项」之间：

| 拒绝 | 位置 | 理由 |
| --- | --- | --- |
| 目标是目录 | `:168` | 硬链接允许成环，目录一旦成环 `.`/`..` 之外就有了走不出的回路；骨架里目录只有一层，这条也是给将来的护栏 |
| 新名字已被占用 | `:172`-`:175` | 两条目录项指向同一个 inode 时，`unlink` 摘掉一条并不能说明摘的是哪一条，账就乱了 |

通过后写目录项的那一段（`:176`-`:186`）与 `create` 完全同构：按 `size / DIRENT_SZ`
数出已有几条、`increase_size` 扩到 `(n+1)*DIRENT_SZ`、在末尾槽位写入
`DirEntry::new(new_name, target.id)`。差别只在最后一行：

```rust
// :187
target.modify_disk_inode(|disk_inode| disk_inode.nlink += 1);
```

`target` 的 `size`、数据块、offset 一个都没碰——这正是「两个名字共享同一个文件」
在实现层面的含义：`ch6_file2` 在 `link` 之后才 `write(fd, "Hello, world!")`，
再 `close`、`unlink(fname)`、`open(lname0)` 读回的仍是那 13 个字节（`Test link OK!`，日志 747 行）。

## 4. `unlink`：先摘名字，再减账，最后回收

`vfs.rs:201`，三步分开写，顺序是有讲究的。

**① 找到名字**（`:203`-`:214`）：线性扫目录项，比对 `dirent.name()`，
拿到 `(slot, victim_id)`。找不到就返回 `false`，`sys_unlinkat` 翻成 `-1`。

**② 摘掉名字**（`:219`-`:238`）：把目录的**最后一条**搬到 `slot` 上，`size -= DIRENT_SZ`。
这就是 swap-remove，不是「整体前移」：

```rust
// :220-225
let last = (dir_inode.size as usize) / DIRENT_SZ - 1;
if slot != last { /* 读走末尾，写进 slot */ }
```

目录项的顺序没有任何语义（`find`/`ls` 都是全表扫），为了保住顺序而把
`last - slot` 之后的每条记录逐条前移（每条一次读一次写，最坏接近 450 条），
换不到任何东西；swap 本来只需一次读一次写。
跨 16 条目录项的块边界还要处理：
条目数从 17 变 16 时末尾那块目录块空了出来（`:226`-`:237` 比对新旧 `data_blocks()`，
把多出来的块指针收集起来交给 `fs.dealloc_data`）。这里只遍历 `direct[]`，
因为本分支的目录不会长到 indirect1：16 条/块 × 28 个直接块 = 448 条目录项。

顺带说清楚这段代码在判分里的覆盖情况：判分镜像的根目录开局就有 **52 条**目录项
（fuse 打了 52 个 ELF，见 lab1 §1），本章测例最多同时挂到 56 条
（`ch6_file2` 的 `fname2` + 三个 `linkname`），全程停在 4 块之内，
从没跨过 16 条/块的边界——也就是说 `:229`-`:236` 这个回收分支在 CI 里走不到，
属于「必须写对但判分不查」的那一段。我按 17→16 这一档单独推演过：
`size` 从 544 变 512，`data_blocks()` 从 2 变 1，循环取 `idx = 1`，
把 `direct[1]` 还进位图并清零；如果漏掉这一步，被摘掉的那条目录项所占的块
就永久从 data bitmap 里挂着，`ch6_file3` 那种反复建删的循环迟早把盘借光。

**③ 给 inode 减账**（`:242`-`:259`）：

```rust
assert!(disk_inode.nlink > 0, "a named inode with no links");
disk_inode.nlink -= 1;
if disk_inode.nlink == 0 {
    (true, disk_inode.clear_size(&self.block_device))
} else { (false, Vec::new()) }
```

归零才 `clear_size` + `dealloc_data` + `dealloc_inode`（`efs.rs:137`，骨架里只有 `alloc_inode`）。
那句 `assert` 是账本的护栏：既然这个名字还挂在目录里，`nlink` 就不该已经是 0。

`ch6_file3` 的 10 轮 `open(CREATE) → 写 2900 字节 → close → unlink → open(RDONLY) 必须失败`
压的是两件事。当场会炸的是**名字必须消失**：`assert!(fd < 0)` 只看 `find("fname3")`，
swap-remove 要是搬错了末尾条目、或忘了把目录 `size` 减掉一个条目，
残留的那半条目录项就会被下一轮的 `open(RDONLY)` 读到，断言立刻失败。
10 轮之内**回收**还测不出来——每轮借走 1 个 inode + 6 个数据块
（58 × 50 = 2900，⌈2900/512⌉ = 6），共 60 块，判分镜像的 data area 有 12628 块
（lab1 §1），要跑到两千多轮才会把 `alloc_data().unwrap()` 逼到 panic。
第 ③ 步归还之后，十轮结束时两块位图回到与第 0 轮相同的状态。
实测日志 750-759 行是 `test iteration 0..9` 连续十行，760 行 `Test mass open/unlink OK!`，没有 panic。

## 5. `stat` 走 `File` trait 的默认方法，不走 `as_any`

`fstat` 要问的字段（`dev/ino/mode/nlink`）只有 `OSInode` 知道，
但 `fd_table` 里存的是 `Arc<dyn File>`。常见写法是给 `File` 加 `as_any` 再 `downcast_ref`。
本分支反过来，在 trait 上给一个带默认实现的方法（`os/src/fs/mod.rs:22`）：

```rust
fn stat(&self) -> Option<Stat> { None }
```

管道/标准输入这类不在盘上的东西**什么都不必写**就得到「没有状态可报」，
`sys_fstat` 据此回 `-1`；`OSInode` 只覆盖这一个方法（`fs/inode.rs:188`）：

| 字段 | 值 | 从哪来 |
| --- | --- | --- |
| `dev` | `0` | 只有一块盘，`BlockDevice` 没有编号概念 |
| `ino` | `inode.inode_id() as u64` | 第 2 节那个 `id` 字段 |
| `mode` | `DIR`/`FILE` | `inode.is_directory()`（`vfs.rs:44`），现读磁盘 |
| `nlink` | `inode.link_count()` | `DiskInode.nlink`（第 1 节），现读磁盘 |
| `pad` | `[0; 7]` | user_lib 的 `Stat` 尾部保留字段，结构体总宽 80 字节 |

四个值全部现场读盘、不缓存，这不是浪费而是 `ch6_file2` 的硬性要求：
`fd` 是 `link` 之前 `open` 到的，`nlink` 却在 `link` 之后问——缓存下来的那份
永远停在 1。代价只有 `fstat` 频率下可忽略的一次块缓存访问。

`sys_fstat`（`syscall/fs.rs:85`）另两点：
`inner.fd_table.get(fd)` 而不是下标——`fd` 是用户给的，越界必须是 `-1` 而不是 panic；
把 80 字节交给用户则是逐字节 `translated_refmut`（`:105`-`:109`），
与 lab2 §1 里 `TimeVal` 那 16 字节同一个手法：调用方的 `Stat` 可能在栈上跨页，
两页的帧并不相邻，整体一次翻译不作保证。

`sys_linkat`（`:117`）/`sys_unlinkat`（`:136`）都很薄：`translated_str` 拿名字，
交给 `link_file`/`unlink_file`（`fs/inode.rs:133/145`），布尔值翻成 `0`/`-1`。
`AT_FDCWD` 那个 `-100` 的 dirfd 参数在 user_lib 的包装里被丢掉，
本内核只有一个扁平根目录，没必要为它编一个「支持但恒等于根」的分支。

## 6. 一处已知边界

`unlink` 掉最后一个名字时，若还有进程握着这个文件的 `fd`，
本实现照样回收 inode 与数据块——之后对该 `fd` 的 `read`/`fstat`
读到的是一个位图已清零、内容仍留在槽位的旧 `DiskInode`（`nlink` 已是 0，
不会 panic，但值没有定义）。POSIX 的语义在这里要求「打开的 descriptor 让文件保持存活」，
需要 `OSInode` 自己持有一份引用计数才能做对。
`ch6_file2` 在 `unlink(lname0)` 之后只 `println!`，源码注释也写了
"It's Ok if you don't delete the inode and data blocks"——
测试对这一步的要求是不出错，本实现满足；把它做成正经语义超出了本章测例的范围，
所以留在这里说明，不假装已经解决。

## 7. 实测读数

判分副本 `oscamp2/c_run/ch6` 的同一次运行（`oscamp2/logs/c_ch6_link1.log`）：

```
743: Usertests: Running ch6_file1      744: Test fstat OK21749!
746: Usertests: Running ch6_file2      747: Test link OK21749!
749: Usertests: Running ch6_file3      750..759: test iteration 0..9
760: Test mass open/unlink OK21749!
778/793/795: [PASS] found <Test link …> / <Test fstat …> / <Test mass open/unlink …>
```

同一份日志 802 行是 `Test passed21749: 31/31`。报告补齐后又跑了一轮
（`oscamp2/logs/c_ch6_final.log`，`randomize` 把 token 换成 32106，743-760 行除 token 外一字不差），
该轮 802 行仍是 31/31、809 行 `MAKE_RC=0`。
