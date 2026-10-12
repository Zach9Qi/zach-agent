//! 阻塞式文件操作，由调用方放到阻塞线程池执行；Unix 上新建的目录权限 0700、文件 0600

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::SystemTime;

/// 读取文件头时最多读取的字节数，避免损坏文件的超长首行
const MAX_HEADER_LEN: u64 = 64 * 1024;

/// 向前查找最后一个换行符时每次读取的块大小
const SCAN_CHUNK: usize = 8 * 1024;

/// 创建目录（含缺失的父目录）
pub(super) fn create_dir(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// 新建文件的打开选项：仅属主可读写
fn private(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(options, 0o600);
    options
}

/// 新建会话文件并写入文件头；文件已存在时返回 `AlreadyExists`
pub(super) fn create_file(path: &Path, header: &[u8]) -> io::Result<File> {
    let mut file =
        private(OpenOptions::new().read(true).write(true).create_new(true)).open(path)?;
    if let Err(error) = file.write_all(header) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

/// 打开已有会话文件用于追加：先截掉末尾没有换行符的残行，再把写入位置移到末尾
///
/// 文件中连一个完整行都没有（文件头不完整）时返回 `None`，此时不做任何修改。
pub(super) fn open_for_append(path: &Path) -> io::Result<Option<File>> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    let complete = complete_len(&mut file, len)?;
    if complete == 0 {
        return Ok(None);
    }
    if complete < len {
        file.set_len(complete)?;
    }
    file.seek(SeekFrom::Start(complete))?;
    Ok(Some(file))
}

/// 文件中到最后一个换行符（含）为止的长度：从末尾按块向前查找
fn complete_len(file: &mut File, len: u64) -> io::Result<u64> {
    let mut buffer = vec![0; SCAN_CHUNK];
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(SCAN_CHUNK as u64);
        let chunk = &mut buffer[..usize::try_from(end - start).map_err(io::Error::other)?];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(chunk)?;
        if let Some(index) = chunk.iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

/// 在当前位置写入一批完整的行；失败时截回写入前的长度，一批条目要么完整写入要么没有
pub(super) fn append(file: &mut File, data: &[u8]) -> io::Result<()> {
    let start = file.stream_position()?;
    let written = file.write_all(data);
    if written.is_err() {
        let _ = file.set_len(start);
        let _ = file.seek(SeekFrom::Start(start));
    }
    written
}

/// 读取文件的第一行
pub(super) fn read_first_line(path: &Path) -> io::Result<Vec<u8>> {
    let mut reader = BufReader::new(File::open(path)?.take(MAX_HEADER_LEN));
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    Ok(line)
}

/// 写入按内容寻址的大对象：已存在则跳过；否则先写临时文件并刷盘，再改名为最终路径，
/// 进程中途被杀也不会在最终路径留下半截内容
pub(super) fn write_blob(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let path = dir.join(name);
    if path.try_exists()? {
        return Ok(());
    }
    let random = RandomState::new().hash_one(SystemTime::now());
    let temp = dir.join(format!(".{name}.{random:016x}.tmp"));
    let written = write_synced(&temp, data).and_then(|()| fs::rename(&temp, &path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        // 并发写入同一内容时，另一方可能已经改名成功
        return if path.try_exists()? {
            Ok(())
        } else {
            Err(error)
        };
    }
    Ok(())
}

fn write_synced(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = private(OpenOptions::new().write(true).create_new(true)).open(path)?;
    file.write_all(data)?;
    file.sync_all()
}
