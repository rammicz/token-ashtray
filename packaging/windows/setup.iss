; Inno Setup 6.5.4. The application and setup code are MIT-licensed; the unmodified
; installer engine retains its upstream copyright and About information.
#if Ver != EncodeVer(6, 5, 4)
  #error Use the reviewed Inno Setup 6.5.4 compiler
#endif
#ifndef PackageDir
  #error PackageDir must name the validated package staging directory
#endif
#ifndef AppVersion
  #error AppVersion is required
#endif

[Setup]
AppId={{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}
AppName=Token Ashtray
AppVersion={#AppVersion}
AppPublisher=rammi
DefaultDirName={localappdata}\Programs\Token Ashtray
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
DisableProgramGroupPage=yes
DisableDirPage=auto
UsePreviousAppDir=yes
UninstallDisplayIcon={app}\token-ashtray.exe
UninstallFilesDir={app}\.setup
OutputBaseFilename=Token-Ashtray-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
RestartApplications=no
SetupLogging=yes

[Files]
Source: "{#PackageDir}\token-ashtray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\.token-ashtray-install"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\.token-ashtray-setup"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\install-windows.ps1"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\install-windows.ps1"; Flags: dontcopy
Source: "{#PackageDir}\LICENSE"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\THIRD-PARTY.md"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\LICENSE-REVIEW.md"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\INNO-LICENSE.txt"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\RELEASE-METRICS.json"; DestDir: "{app}\.setup"; Flags: ignoreversion
Source: "{#PackageDir}\INSTALL-WINDOWS.md"; DestDir: "{app}\.setup"; Flags: ignoreversion

[Icons]
; The helper validates and removes this exact shortcut on uninstall; Inno's
; unconditional shortcut deletion would remove a later unrelated replacement.
Name: "{userprograms}\Token Ashtray"; Filename: "{app}\token-ashtray.exe"; WorkingDir: "{app}"; Comment: "Token Ashtray per-user launcher"; Flags: uninsneveruninstall

[Code]
const
  // The app's instance guard (src/platform/instance.rs). It exists while the
  // tray host or any of its board processes runs.
  AppGuard = 'Local\TokenAshtray-{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}';
  // The app's updater passes /WAITAPP and starts setup just before it exits.
  WaitAppSwitch = '/WAITAPP';
  AppExitTimeoutSeconds = 60;
  // Held while a setup runs. A second setup started meanwhile, for example
  // by another Quit while the first one waits, changes nothing.
  SetupGuard = 'Local\TokenAshtraySetup-{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}';
  ERROR_ALREADY_EXISTS = 183;
  // Beside its log, a refused setup leaves this marker, so the app does not
  // count that start as a failed attempt (src/updates/store.rs).
  RefusedExtension = '.refused';

var
  // Kept open until setup exits, which releases the guard.
  SetupGuardHandle: Cardinal;

function GetFileAttributes(FileName: String): Cardinal;
  external 'GetFileAttributesW@kernel32.dll stdcall';

function CreateMutexW(Attributes: Cardinal; InitialOwner: Integer; Name: String): Cardinal;
  external 'CreateMutexW@kernel32.dll stdcall';

function HasSwitch(Switch: String): Boolean;
var
  Index: Integer;
begin
  Result := False;
  for Index := 1 to ParamCount do
    if CompareText(ParamStr(Index), Switch) = 0 then
      Result := True;
end;

// Marks this start as refused, next to the log the app asked for.
procedure MarkRefused;
var
  LogFile: String;
begin
  LogFile := ExpandConstant('{param:LOG|}');
  // Whether or not Inno kept the quotes of /LOG="...", drop them.
  StringChangeEx(LogFile, '"', '', True);
  if LogFile <> '' then
    SaveStringToFile(ChangeFileExt(LogFile, RefusedExtension),
      'Another Token Ashtray setup was running.', False);
end;

// Creates the setup guard, or learns that it exists, in one Windows call, so
// two setups started at the same moment cannot both continue.
function InitializeSetup: Boolean;
begin
  SetupGuardHandle := CreateMutexW(0, 0, SetupGuard);
  if SetupGuardHandle = 0 then begin
    Result := False;
    SuppressibleMsgBox('Token Ashtray setup could not start. Nothing was changed.',
      mbError, MB_OK, IDOK);
  end else if DLLGetLastError = ERROR_ALREADY_EXISTS then begin
    Result := False;
    MarkRefused;
    SuppressibleMsgBox('Token Ashtray setup is already running. Nothing was changed.',
      mbError, MB_OK, IDOK);
  end else
    Result := True;
end;

// With /WAITAPP, wait until every app process has exited, so no file is in
// use. If the app is still running at the timeout, nothing is changed.
function WaitForAppExit: String;
var
  Tenths: Integer;
begin
  Result := '';
  if not HasSwitch(WaitAppSwitch) then
    Exit;
  Tenths := 0;
  while CheckForMutexes(AppGuard) and (Tenths < AppExitTimeoutSeconds * 10) do begin
    Sleep(100);
    Tenths := Tenths + 1;
  end;
  if CheckForMutexes(AppGuard) then
    Result := 'Token Ashtray is still running after ' + IntToStr(AppExitTimeoutSeconds) +
      ' seconds. Nothing was changed.'
  else
    Log('Token Ashtray has exited after ' + IntToStr(Tenths * 100) + ' ms.');
end;

function SetupResourcesError: String;
var
  Directory: String;
  Attributes: Cardinal;
  Marker: AnsiString;
  FindRec: TFindRec;
begin
  Result := '';
  Directory := ExpandConstant('{app}\.setup');
  Attributes := GetFileAttributes(Directory);
  if Attributes = $FFFFFFFF then
    Exit;
  if (Attributes and $410) <> $10 then begin
    Result := 'Setup resources must be an ordinary directory: ' + Directory;
    Exit;
  end;
  if not LoadStringFromFile(Directory + '\.token-ashtray-setup', Marker) or
      (Marker <> 'Token Ashtray graphical setup resources' + #10) then begin
    Result := 'Setup resources belong to another installation: ' + Directory;
    Exit;
  end;
  if FindFirst(Directory + '\*', FindRec) then begin
    try
      repeat
        if (FindRec.Name <> '.') and (FindRec.Name <> '..') and
            ((FindRec.Attributes and $410) <> 0) then begin
          Result := 'Setup resources contain a directory or link: ' + Directory + '\' + FindRec.Name;
          Exit;
        end;
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
end;

function RunHelper(Script, Action: String; ValidateOnly: Boolean): String;
var
  Parameters: String;
  ExitCode, Index: Integer;
  Output: TExecOutput;
begin
  Parameters := '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File ' +
    AddQuotes(Script) + ' -Action ' + Action + ' -InstallDir ' + AddQuotes(ExpandConstant('{app}'));
  if ValidateOnly then
    Parameters := Parameters + ' -ValidateOnly';
  Result := '';
  if not ExecAndCaptureOutput(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      Parameters, '', SW_HIDE, ewWaitUntilTerminated, ExitCode, Output) then begin
    Result := 'Could not run the installation checks. Windows error: ' + IntToStr(ExitCode);
    Exit;
  end;
  for Index := 0 to GetArrayLength(Output.StdOut) - 1 do
    Log(Output.StdOut[Index]);
  for Index := 0 to GetArrayLength(Output.StdErr) - 1 do begin
    Log(Output.StdErr[Index]);
    Result := Result + Output.StdErr[Index] + #13#10;
  end;
  if (ExitCode = 0) and not Output.Error then
    Result := ''
  else if Result = '' then
    Result := 'Installation checks failed (exit ' + IntToStr(ExitCode) + '). No further changes were made.';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := WaitForAppExit;
  if Result <> '' then
    Exit;
  Result := SetupResourcesError;
  if Result <> '' then
    Exit;
  ExtractTemporaryFile('install-windows.ps1');
  Result := RunHelper(ExpandConstant('{tmp}\install-windows.ps1'), 'Install', True);
end;

function InitializeUninstall: Boolean;
var
  Error: String;
begin
  Error := SetupResourcesError;
  if Error = '' then
    Error := RunHelper(ExpandConstant('{app}\.setup\install-windows.ps1'), 'Uninstall', True);
  Result := Error = '';
  if not Result then
    SuppressibleMsgBox(Error, mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Error: String;
begin
  if CurUninstallStep = usUninstall then begin
    // Run only after the user's uninstall confirmation. A refusal here is a
    // fatal Inno event, before PerformUninstall removes files or registration.
    Error := RunHelper(ExpandConstant('{app}\.setup\install-windows.ps1'), 'Uninstall', False);
    if Error <> '' then
      RaiseException(Error);
  end;
end;
