; 安装器钩子：目录定位 + 旧版本处理
;
; 背景（两次真实事故）：
;   ① Tauri 的 NSIS 模板用 `Software\<publisher>\<product>` 的默认值记住上次安装目录
;      （RestorePreviousInstallLocation）。v1.3.7 把 publisher 从 WorkBuddy 改成 qvq9999 后
;      这个键换了名字 —— 旧记录留在 `Software\WorkBuddy\disk-manager`，新键是空的，
;      安装器回落到 `C:\Program Files\disk-manager`，装在 D 盘的用户被装成两份。
;   ② 覆盖安装时安装器会先调旧卸载器（`uninstall.exe _?=$4`，$4 来自上面那个键）。
;      键是空的时候 `_?=` 也是空；再加上旧版本还开着（进程占用），卸载器删不掉 app.exe，
;      安装器就断言 `$INSTDIR\app.exe` 仍然存在 → 弹「无法卸载！」并中止安装。
;
; 所以这里在 GUI 初始化阶段（早于所有页面与卸载流程）做三件事：
;   ① 把旧 publisher 的位置记录迁移到当前键，保证 `$4` 有值
;   ② 修正 `$INSTDIR`：当前运行目录 > 已记录的安装位置 > 全新安装默认 D 盘
;   ③ 关掉仍在运行的旧版本，否则卸载/覆盖都会失败
;
; ⚠️ 本文件被 `!include` 在模板**很靠前**的位置（早于 `!define PRODUCTNAME`），
;    因此 Function 体内不能使用 `${PRODUCTNAME}` 这类后续 define，只能用字面量。
;    目录名必须与 `tauri.conf.json` 里的产品名保持一致。

!include "LogicLib.nsh"

!define DM_PRODUCT_DIR "disk-manager"
!define DM_META_KEY "Software\qvq9999\${DM_PRODUCT_DIR}"

; 不能自己定义 `.onGUIInit` —— MUI2 已占用，重复定义会报
; `Function named ".onGUIInit" already exists`。MUI2 提供下面这个钩子供挂载。
!define MUI_CUSTOMFUNCTION_GUIINIT dmGuiInit

Function dmGuiInit
  ; ---------- ① 迁移旧 publisher 留下的安装位置 ----------
  ReadRegStr $0 HKLM "${DM_META_KEY}" ""
  ${If} $0 == ""
    ; 卸载项登记的安装位置最权威（系统与卸载器都按它走），值带引号需去掉
    ReadRegStr $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${DM_PRODUCT_DIR}" "InstallLocation"
    StrCpy $1 $0 1
    ${If} $1 == '"'
      StrCpy $0 $0 -1 1
      StrCpy $1 $0 1 -1
      ${If} $1 == '"'
        StrCpy $0 $0 -1
      ${EndIf}
    ${EndIf}
    ; 再退回 publisher 改名前的键
    ${If} $0 == ""
      ReadRegStr $0 HKLM "Software\WorkBuddy\${DM_PRODUCT_DIR}" ""
    ${EndIf}
    ; 写回当前键 —— 卸载阶段读的就是它
    ${If} $0 != ""
      WriteRegStr HKLM "${DM_META_KEY}" "" "$0"
      WriteRegStr HKCU "${DM_META_KEY}" "" "$0"
    ${EndIf}
  ${EndIf}

  ; ---------- ② 决定安装目录 ----------
  ; 只在「默认目录里没有主程序」时重新定位：已经装在这个位置就原地不动，
  ; 升级不能把程序从用户放它的地方挪走。
  ${IfNot} ${FileExists} "$INSTDIR\app.exe"
    ; 首选：应用记录的「当前从哪运行」（在哪打开就更到哪）
    ReadRegStr $2 HKCU "${DM_META_KEY}" "RunningFrom"
    ${If} $2 == ""
      ReadRegStr $2 HKLM "${DM_META_KEY}" "RunningFrom"
    ${EndIf}
    ${If} $2 != ""
    ${AndIf} ${FileExists} "$2\app.exe"
      StrCpy $INSTDIR "$2"
    ${Else}
      ; 其次：注册表里记的上次安装位置（① 已迁移好）
      ${If} $0 != ""
      ${AndIf} ${FileExists} "$0\app.exe"
        StrCpy $INSTDIR "$0"
      ${Else}
        ; 最后：全新安装，默认装 D 盘（没有 D 盘才留在默认的 C 盘）
        ${GetFileName} "$INSTDIR" $3
        ${If} $3 == ""
          StrCpy $3 "${DM_PRODUCT_DIR}"
        ${EndIf}
        ${If} ${FileExists} "D:\*.*"
          StrCpy $INSTDIR "D:\$3"
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ; ---------- ③ 关掉仍在运行的旧版本 ----------
  ; 必须在卸载之前。安装器内置的 CheckIfAppIsRunning 在 Section 里执行，
  ; 而「卸载旧版」发生在页面回调阶段 —— 那时程序还开着，卸载器删不掉 app.exe，
  ; 紧接着就是「无法卸载！」。RestartManager 按文件路径匹配，不会误伤同名程序。
  !insertmacro RestartManager_StartSession $R0
  ${If} $R0 != ""
    !insertmacro RestartManager_RegisterFile $R0 "$INSTDIR\app.exe"
    ${If} $0 = 0
      System::Call 'RSTRTMGR::RmGetList(p R0, *i .r1, *i .r2, p 0, *i .r3) i .r0'
      ; 234 = ERROR_MORE_DATA，表示确实有进程占着这个文件
      ${If} $0 = 234
        IfSilent dm_kill_done
        MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
          "检测到 $(^Name) 正在运行，需要先关闭才能继续安装。$\r$\n$\r$\n点「确定」自动关闭并继续，点「取消」退出安装。" \
          IDOK dm_kill
        !insertmacro RestartManager_EndSession $R0
        Quit
        dm_kill:
        System::Call 'RSTRTMGR::RmShutdown(p R0, i ${RmForceShutdown}, p 0) i .r0'
      ${EndIf}
    ${EndIf}
    dm_kill_done:
    !insertmacro RestartManager_EndSession $R0
  ${EndIf}
FunctionEnd

; 静默安装（应用内更新，/UPDATE）不会执行 GUIInit，这里再判一次目录。
; 此时卸载流程不会走（/UPDATE 直接覆盖），所以只需保证 $INSTDIR 正确。
!macro NSIS_HOOK_PREINSTALL
  ; Section 里已经 SetOutPath 过一次，改完 $INSTDIR 必须重新设置输出目录，
  ; 否则 File 仍会写进原来的位置。

  ; ① 正在使用的目录。应用写在 HKCU（不需要提权，一定写得进去），HKLM 作兜底
  ReadRegStr $0 HKCU "${DM_META_KEY}" "RunningFrom"
  ${If} $0 == ""
    ReadRegStr $0 HKLM "${DM_META_KEY}" "RunningFrom"
  ${EndIf}
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\${MAINBINARYNAME}.exe"
    ${If} $0 != "$INSTDIR"
      StrCpy $INSTDIR "$0"
      SetOutPath "$INSTDIR"
    ${EndIf}
    Goto dm_hook_dir_done
  ${EndIf}

  ; ② 默认位置本来就有程序：上次装在这里，原地更新
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    Goto dm_hook_dir_done
  ${EndIf}

  ; ③ 旧 publisher 记录的位置（publisher 改名前的安装）
  ReadRegStr $0 HKLM "Software\WorkBuddy\disk-manager" ""
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\${MAINBINARYNAME}.exe"
    StrCpy $INSTDIR "$0"
    SetOutPath "$INSTDIR"
    Goto dm_hook_dir_done
  ${EndIf}

  ; ④ 卸载项里登记的安装位置（带引号，需要去掉）
  ReadRegStr $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "InstallLocation"
  StrCpy $1 $0 1
  ${If} $1 == '"'
    StrCpy $0 $0 -1 1
    StrCpy $1 $0 1 -1
    ${If} $1 == '"'
      StrCpy $0 $0 -1
    ${EndIf}
  ${EndIf}
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\${MAINBINARYNAME}.exe"
    StrCpy $INSTDIR "$0"
    SetOutPath "$INSTDIR"
  ${EndIf}

  dm_hook_dir_done:
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; 记下这次装到哪：下次更新直接用它，不再依赖 publisher 名字拼出来的键
  WriteRegStr HKCU "${DM_META_KEY}" "RunningFrom" "$INSTDIR"
  WriteRegStr HKLM "${DM_META_KEY}" "RunningFrom" "$INSTDIR"
  ; 通知应用：本次安装完成后清理「另一份旧安装」
  WriteRegDWORD HKLM "${DM_META_KEY}" "CleanOthers" 1
  WriteRegDWORD HKCU "${DM_META_KEY}" "CleanOthers" 1
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 卸载完成后位置记录已无意义，避免下次安装又被指向已删除的目录
  DeleteRegValue HKCU "${DM_META_KEY}" "RunningFrom"
  DeleteRegValue HKCU "${DM_META_KEY}" "CleanOthers"
  DeleteRegValue HKLM "${DM_META_KEY}" "RunningFrom"
  DeleteRegValue HKLM "${DM_META_KEY}" "CleanOthers"
!macroend
