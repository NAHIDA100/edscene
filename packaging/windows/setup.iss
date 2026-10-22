; edscene Windows 安装向导（Inno Setup 6）
; 由 packaging/windows/pack.ps1 调用：
;   ISCC /DStageDir=<暂存目录> /DReleaseDir=<输出目录> /DAppVersion=0.1.2 setup.iss
; 暂存目录里有 edscene.exe、sounds\、background.jpg、README/许可/NOTICE 与 docs。
; 素材不在仓库里，所以这一步只在本地跑。
; 三个符号都由命令行 /D 传入；脚本里再 #define 一次会把传进来的值清空。

[Setup]
AppId={{7B4E2C1D-9F3A-4C6B-8E5D-2A1C4D7E9F0B}
AppName=集成工业播报终端
AppVersion={#AppVersion}
AppVerName=集成工业播报终端 {#AppVersion}
AppPublisher=edscene contributors
DefaultDirName={autopf}\edscene
DefaultGroupName=集成工业播报终端
DisableProgramGroupPage=yes
DisableWelcomePage=no
WizardStyle=modern
LicenseFile=installer-notice.txt
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=6.2sp2
AllowRootDirectory=no
AllowUNCPath=no
UninstallDisplayName=集成工业播报终端
UninstallFilesDir={app}
OutputDir={#ReleaseDir}
OutputBaseFilename=edscene-setup-{#AppVersion}-windows-x64
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "zh"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Tasks]
Name: "autostart"; Description: "开机自启（登录界面之前就加载服务）"; GroupDescription: "附加任务："
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "service-control.ps1"; Flags: dontcopy

[Icons]
Name: "{autoprograms}\集成工业播报终端"; Filename: "{app}\edscene.exe"
Name: "{autodesktop}\集成工业播报终端"; Filename: "{app}\edscene.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\edscene.exe"; Description: "打开设置窗口"; Flags: nowait postinstall skipifsilent runascurrentuser

[Code]
var
  StoppedOld: Boolean;
  InstalledOk: Boolean;

function StatePath: String;
begin
  Result := ExpandConstant('{tmp}\edscene-service-state.txt');
end;

function RunCtl(const Mode: String): Integer;
var
  Code: Integer;
  Args: String;
begin
  Args := '-NoProfile -ExecutionPolicy Bypass -File "' + ExpandConstant('{tmp}\service-control.ps1') +
    '" -Mode ' + Mode + ' -InstallDir "' + ExpandConstant('{app}') + '" -StateFile "' + StatePath + '"';
  if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'), Args, '',
      SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := 1
  else
    Result := Code;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  NeedsRestart := False;
  InstalledOk := False;
  if not FileExists(ExpandConstant('{app}\edscene.exe')) then Exit;
  ExtractTemporaryFile('service-control.ps1');
  StoppedOld := True;
  if RunCtl('Stop') <> 0 then
  begin
    StoppedOld := False;
    Result := '没能停掉正在运行的旧版本，安装已中止，没有改动任何文件。' + #13#10 +
      '详情见 %TEMP%\Setup Log*.txt（脚本只处理指向本安装目录的服务与进程）。';
  end;
end;

function ServiceLogFile: String;
begin
  Result := ExpandConstant('{commonappdata}\edscene\service-install.log');
end;

// 子进程是 GUI 子系统，向导又是隐藏窗口跑的它，所以退出码背后那句真正的报错
// 谁都不会看见。程序自己把这行写进日志，这里只负责把日志递到用户手上。
procedure ExplainServiceFailure(const What: String; Code: Integer);
var
  Answer, Dummy: Integer;
  Text: String;
begin
  Text := '程序文件已经装好，但这一步没完成：' + What + '（退出码 ' + IntToStr(Code) + '）。' + #13#10#13#10;
  if FileExists(ServiceLogFile) then
    Text := Text + '失败的原因只有程序自己写得出来，它已经记在这一行里：' + #13#10 +
      ServiceLogFile + #13#10#13#10 +
      '现在用记事本打开它吗？把最后几行发给开发者就能定位问题。' + #13#10 +
      '也可以先在管理员命令行里执行 edscene --install-service 重试。'
  else
    Text := Text + '诊断日志还没生成（预期位置 ' + ServiceLogFile + '）。' + #13#10 +
      '请在管理员命令行里执行 edscene --install-service，把它的输出发出来。';
  Answer := MsgBox(Text, mbError, MB_YESNO);
  if FileExists(ServiceLogFile) and (Answer = IDYES) then
    Exec('notepad.exe', '"' + ServiceLogFile + '"', '', SW_SHOW, ewNoWait, Dummy);
end;

procedure RegisterService;
var
  Code: Integer;
begin
  if not Exec(ExpandConstant('{app}\edscene.exe'), '--install-service', '',
      SW_HIDE, ewWaitUntilTerminated, Code) then Code := 1;
  if Code <> 0 then
  begin
    ExplainServiceFailure('注册开机自启服务', Code);
    Exit;
  end;
  InstalledOk := True;
  if not WizardIsTaskSelected('autostart') then
  begin
    if not Exec(ExpandConstant('{app}\edscene.exe'), '--autostart-off', '',
        SW_HIDE, ewWaitUntilTerminated, Code) then Code := 1;
    if Code <> 0 then
      ExplainServiceFailure('按你的选择关掉开机自启', Code);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then RegisterService;
end;

procedure DeinitializeSetup;
begin
  // 停在"已停旧服务、但没装成"的中间状态是最糟的：把启动类型还回去。
  if StoppedOld and not InstalledOk then
    RunCtl('Restore');
end;

function InitializeUninstall(): Boolean;
var
  Code: Integer;
begin
  Result := True;
  if not FileExists(ExpandConstant('{app}\edscene.exe')) then Exit;
  ExtractTemporaryFile('service-control.ps1');
  RunCtl('Stop');
  Exec(ExpandConstant('{app}\edscene.exe'), '--uninstall-service', '',
    SW_HIDE, ewWaitUntilTerminated, Code);
  // 配置与替换过的音频在 %PROGRAMDATA%\edscene 与用户目录里，刻意不碰。
end;
