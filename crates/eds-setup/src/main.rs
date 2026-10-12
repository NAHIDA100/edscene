//! The Windows installer: `setup.exe` is this program with the sound pack and
//! the artwork appended to it (see `payload`). Double-click, say yes once, and
//! the program is installed, registered as a service and on the Start Menu.
//!
//! Everything here runs with the user's own credentials until the UAC prompt is
//! accepted; the only elevated actions are copying into Program Files, service
//! registration and the Start Menu entry.

// 非 Windows 上这个二进制只是 --pack / --help 的外壳，安装流程整个不参与编译，
// 所以那些只有 Windows 路径会碰到的辅助函数在这里必然是"未使用"。
#![cfg_attr(not(windows), allow(dead_code, unreachable_code))]

mod payload;

use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

#[cfg(windows)]
mod win;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = r#"edscene-setup —— 集成工业播报台 Windows 安装程序

  setup.exe                  双击安装（会弹一次 UAC）
  setup.exe --yes            不再询问版权确认
  setup.exe --silent         完全没有交互，失败以非零码退出
  setup.exe --dir <路径>      自定义安装目录（默认 %ProgramFiles%\edscene）
  setup.exe --uninstall      卸载：移除服务并删除安装目录（会先问）
  setup.exe --pack <素材目录> <安装器exe> <输出exe>
                             打包用：把素材追加到安装器尾部生成 setup.exe
"#;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => exit(0),
        Err(e) => {
            eprintln!("edscene-setup: {e}");
            exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let flag = |n: &str| args.iter().any(|a| a == n);
    if flag("--help") || flag("-h") {
        print!("{HELP}");
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--pack") {
        return pack(&args[i + 1..]);
    }

    #[cfg(windows)]
    {
        // An interactive run gets a console of its own: the alternative is a
        // window that appears and vanishes before anything can be read.
        if !flag("--silent") {
            win::attach_console();
        }
        if !win::is_elevated() {
            if flag("--elevated") {
                // Never let the child program raise a second UAC box.
                return Err("提权后仍然不是管理员，装不了；请在管理员命令行里重跑".into());
            }
            return win::relaunch_elevated(&reexec_args(args));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        return Err("这个安装程序只支持 Windows；Linux 请用 dpkg 或 tar 包".into());
    }

    banner();
    if flag("--uninstall") {
        return uninstall(flag("--yes") || flag("--silent"));
    }
    let dir = install_dir(args);
    let quiet = flag("--silent");
    if !quiet && !flag("--yes") && !consent() {
        println!("已取消，没有写入任何文件。");
        wait_to_close();
        return Ok(());
    }
    install(&dir, quiet)?;
    if !quiet {
        wait_to_close();
    }
    Ok(())
}

/// `--pack` is the only part that runs un-elevated and writes outside the
/// machine tree, so it never reaches the UAC step.
fn pack(args: &[String]) -> Result<(), String> {
    if args.len() != 3 {
        return Err("--pack 需要三个参数：<素材目录> <安装器exe> <输出exe>".into());
    }
    let (src, host, out) = (Path::new(&args[0]), Path::new(&args[1]), Path::new(&args[2]));
    let files = payload::collect(src)?;
    if files.is_empty() {
        return Err(format!("{src:?} 是空的，没什么可打包"));
    }
    payload::pack(host, &files, out)?;
    let total: u64 = files
        .iter()
        .map(|(_, p)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    println!(
        "已追加 {} 个文件（{} MB）→ {}",
        files.len(),
        total / 1_048_576,
        out.display()
    );
    for (name, _) in &files {
        println!("  · {name}");
    }
    Ok(())
}

/// Keep the user's choices when re-launching elevated.
fn reexec_args(args: &[String]) -> Vec<String> {
    let mut v: Vec<String> = args
        .iter()
        .filter(|a| **a != "--elevated")
        .cloned()
        .collect();
    v.push("--elevated".into());
    v
}

fn banner() {
    println!("集成工业播报台 edscene v{VERSION} — Windows 安装程序");
    println!(
        r#"版权与素材声明
  1. 本程序是《明日方舟：终末地》的同人二创工具，与鹰角网络无隶属关系、无授权
     关系，也不是官方软件。相关的商标与著作权归各自的权利人所有。
  2. 随包分发的语音（sounds\）与背景图（background.jpg）版权归权利人所有，只作
     个人学习、非商业用途。用于商业、二次分发或对外传播都可能构成侵权。
  3. 权利人如认为侵权，开 issue 或发邮件即可，我们会立刻从发行渠道下架。
  4. 装好后随时可以换掉这些素材（设置 → 播报事件 → 替换 / 恢复，设置 → 外观 →
     背景图片）。本程序不保留副本，也不会把任何文件上传到网络。"#
    );
}

fn consent() -> bool {
    print!("看清以上四条，输入 yes 继续（其他任意输入退出）：");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    line.trim().eq_ignore_ascii_case("yes")
}

fn wait_to_close() {
    println!("\n按回车关闭这个窗口。");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

fn install_dir(args: &[String]) -> PathBuf {
    if let Some(i) = args.iter().position(|a| a == "--dir") {
        if let Some(p) = args.get(i + 1) {
            return PathBuf::from(p);
        }
    }
    let pf = std::env::var("ProgramFiles")
        .or_else(|_| std::env::var("PROGRAMFILES"))
        .unwrap_or_else(|_| r"C:\Program Files".into());
    Path::new(&pf).join("edscene")
}

fn install(dir: &Path, quiet: bool) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| format!("找不到自己：{e}"))?;
    let mut c = payload::Container::open(&me)?
        .ok_or_else(|| format!("{} 里没有随包素材；请用发行包里的 setup.exe", me.display()))?;
    println!(
        "\n安装到 {}（{} 个文件，约 {} MB）",
        dir.display(),
        c.entries().len(),
        c.total() / 1_048_576
    );

    let target = dir.join("edscene.exe");
    if target.exists() {
        // The old service and any agent or tray in the user's session all hold
        // this file open; letting go of the lock is the whole upgrade dance.
        let _ = run_sync(&target, &["--uninstall-service"], quiet);
        let _ = run_taskkill(quiet);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    for (n, e) in c.entries().to_vec().iter().enumerate() {
        if !quiet {
            print!("\r  写入 {} / {}", n + 1, c.entries().len());
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        let rel = payload::safe_relpath(&e.name).ok_or_else(|| format!("非法路径 {}", e.name))?;
        let dest = dir.join(rel);
        c.extract(e, &dest)?;
    }
    if !quiet {
        println!();
    }

    let exe = dir.join("edscene.exe");
    if !exe.is_file() {
        return Err("载荷里没有 edscene.exe，安装中止".into());
    }
    std::process::Command::new(&exe)
        .arg("--install-service")
        .status()
        .map_err(|e| format!("无法启动 {}：{e}", exe.display()))?
        .success()
        .then_some(())
        .ok_or_else(|| "服务注册失败：edscene --install-service 返回了非零码".to_string())?;
    println!("服务已注册并启动（开机自启，登录界面就能出声）。");

    #[cfg(windows)]
    match win::start_menu_shortcut(&exe) {
        Ok(p) => println!("开始菜单：{p}"),
        Err(e) => println!("开始菜单快捷方式没建成（{e}）；可以直接运行 {}。", exe.display()),
    }

    println!(
        "\n装好了。托盘里会有「集成工业播报台」图标，右键 → 打开设置；\n\
         没有托盘图标时双击 {} 也一样。",
        exe.display()
    );
    Ok(())
}

fn uninstall(assumed_yes: bool) -> Result<(), String> {
    let dir = install_dir(&[]);
    let exe = dir.join("edscene.exe");
    if !exe.is_file() {
        return Err(format!("{} 不在，看起来没有装过", exe.display()));
    }
    if !assumed_yes {
        print!("将停止并删除服务，然后删除 {} 里的程序文件（配置与替换素材保留）。输入 yes 确认：", dir.display());
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !line.trim().eq_ignore_ascii_case("yes") {
            println!("已取消。");
            return Ok(());
        }
    }
    let _ = run_sync(&exe, &["--uninstall-service"], false);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        println!("服务已卸载，但 {} 没删干净：{e}", dir.display());
    } else {
        println!("已卸载 {}。", dir.display());
    }
    Ok(())
}

/// A child process whose own console output goes straight to the user.
fn run_sync(exe: &Path, args: &[&str], quiet: bool) -> Result<(), String> {
    let mut c = Command::new(exe);
    c.args(args);
    if quiet {
        c.stdout(Stdio::null()).stderr(Stdio::null());
    }
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let s = c.status().map_err(|e| format!("{}: {e}", exe.display()))?;
    if s.success() {
        Ok(())
    } else {
        Err(format!("{} {} 退出码 {s:?}", exe.display(), args.join(" ")))
    }
}

/// Leftovers in the user's session (agent + tray) also lock the exe.
fn run_taskkill(_quiet: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "edscene.exe"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    Ok(())
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;
