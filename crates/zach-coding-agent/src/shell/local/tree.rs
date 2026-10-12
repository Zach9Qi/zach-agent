//! 进程树的隔离与清理：命令被超时或中止时，连同它派生的后台进程一起终止
//!
//! - Unix：子进程自成进程组，向整组发送 SIGKILL；
//! - Windows：子进程放入作业对象（Job Object），子孙进程自动加入同一作业，整体终止。
//!   不依赖父子关系，因此 MSYS `exec` 等打断进程树的情况也能覆盖。
//!   子进程启动后才加入作业，启动瞬间派生的进程理论上可能漏网，实际窗口只有微秒级。

use tokio::process::{Child, Command};

/// Windows 上不为子进程创建控制台窗口
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 让子进程自成一组，便于整组终止
pub(super) fn isolate(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
}

/// 析构时终止整棵进程树；正常退出后应调用 [`Self::disarm`]
///
/// 接管失败（如作业对象创建失败）时退化为只依赖 `kill_on_drop` 终止直接子进程。
pub(super) struct TreeGuard {
    tree: Option<Tree>,
}

impl TreeGuard {
    /// 接管刚启动的子进程
    pub(super) fn attach(child: &Child) -> Self {
        Self {
            tree: Tree::attach(child),
        }
    }

    /// 命令已正常结束：不再清理，保留其有意留在后台的进程
    pub(super) fn disarm(&mut self) {
        if let Some(tree) = self.tree.take() {
            tree.release();
        }
    }
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        if let Some(tree) = self.tree.take() {
            tree.kill();
        }
    }
}

/// 以进程组 ID（即子进程 PID，见 [`isolate`]）标识的进程树
#[cfg(unix)]
struct Tree(i32);

#[cfg(unix)]
impl Tree {
    fn attach(child: &Child) -> Option<Self> {
        child.id().and_then(|pid| i32::try_from(pid).ok()).map(Self)
    }

    fn release(self) {}

    fn kill(self) {
        // SAFETY: kill 只发送信号，不读写任何内存；进程组已不存在时返回错误，忽略即可
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
use job::Job as Tree;

#[cfg(windows)]
mod job {
    use std::io;
    use std::mem::size_of;
    use std::ptr;
    use tokio::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// 被终止进程的退出码
    const KILLED_EXIT_CODE: u32 = 1;

    /// 匿名作业对象；关闭句柄时若仍设置了"关闭即终止"，作业内的进程会一并结束
    pub(super) struct Job(HANDLE);

    // SAFETY: 内核对象句柄不绑定线程，可以在线程间转移
    unsafe impl Send for Job {}

    impl Job {
        pub(super) fn attach(child: &Child) -> Option<Self> {
            let process = child.raw_handle()?;
            let job = Self::create().ok()?;
            // SAFETY: 两个句柄都有效：作业刚创建，进程句柄来自尚未回收的子进程
            let assigned = unsafe { AssignProcessToJobObject(job.0, process as HANDLE) };
            (assigned != 0).then_some(job)
        }

        fn create() -> io::Result<Self> {
            // SAFETY: 空指针表示默认安全属性与匿名作业，返回值在下方检查
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            job.set_kill_on_close(true)?;
            Ok(job)
        }

        fn set_kill_on_close(&self, enabled: bool) -> io::Result<()> {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            if enabled {
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("结构体大小必然放得进 u32");
            // SAFETY: 传入的是该信息类别对应的结构体及其准确大小，句柄有效
            let ok = unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    ptr::from_ref(&info).cast(),
                    size,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// 取消"关闭即终止"后关闭句柄，作业内仍在运行的后台进程得以保留
        pub(super) fn release(self) {
            let _ = self.set_kill_on_close(false);
        }

        /// 终止作业内的全部进程
        pub(super) fn kill(self) {
            // SAFETY: 句柄有效；作业内已无进程时调用无副作用
            unsafe {
                TerminateJobObject(self.0, KILLED_EXIT_CODE);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: 句柄由本对象独占，只在这里关闭一次
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
