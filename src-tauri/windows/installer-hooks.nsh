; 安装器钩子：保证「更新」装回原来那个目录，而不是在 C 盘另起一份。
;
; 背景（v1.3.8 修复的真实事故）：
;   Tauri 的 NSIS 模板用 `Software\<publisher>\<product>` 的默认值记住上次安装目录
;   （RestorePreviousInstallLocation）。v1.3.7 把 publisher 从 WorkBuddy 改成 qvq9999 后，
;   这个键换了一个名字 —— 旧记录留在 `Software\WorkBuddy\disk-manager`，新键是空的，
;   安装器于是回落到默认的 `C:\Program Files\disk-manager`。用户原来装在 D 盘，
;   结果变成 D 盘旧版 + C 盘新版两份并存，注册表只指向新的那份。
;
; 判定顺序（先到先得，每个候选都必须真的存在主程序才采纳）：
;   ① RunningFrom        应用每次启动写入的「当前从哪运行」—— 1.3.8 之后最准
;   ② $INSTDIR 已有程序  上次就装在这里，保持不动（已装了两份的机器靠这条原地更新）
;   ③ 旧 publisher 的键  从 1.3.6 及更早升上来的第一次
;   ④ 卸载项 InstallLocation
; 这样既不会把程序"搬走"，也不会在已装两份的机器上再制造第三份。

!include "LogicLib.nsh"

!macro NSIS_HOOK_PREINSTALL
  ; Section 里已经 SetOutPath 过一次，改完 $INSTDIR 必须重新设置输出目录，
  ; 否则 File 仍会写进原来的位置。

  ; ① 正在使用的目录。应用写在 HKCU（不需要提权，一定写得进去），HKLM 作兜底
  ReadRegStr $0 HKCU "Software\qvq9999\disk-manager" "RunningFrom"
  ${If} $0 == ""
    ReadRegStr $0 HKLM "Software\qvq9999\disk-manager" "RunningFrom"
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
  WriteRegStr HKCU "Software\qvq9999\disk-manager" "RunningFrom" "$INSTDIR"
  WriteRegStr HKLM "Software\qvq9999\disk-manager" "RunningFrom" "$INSTDIR"
  ; 通知应用：本次安装完成后清理「另一份旧安装」
  WriteRegDWORD HKLM "Software\qvq9999\disk-manager" "CleanOthers" 1
  WriteRegDWORD HKCU "Software\qvq9999\disk-manager" "CleanOthers" 1
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 卸载完成后位置记录已无意义，避免下次安装又被指向已删除的目录
  DeleteRegValue HKCU "Software\qvq9999\disk-manager" "RunningFrom"
  DeleteRegValue HKCU "Software\qvq9999\disk-manager" "CleanOthers"
  DeleteRegValue HKLM "Software\qvq9999\disk-manager" "RunningFrom"
  DeleteRegValue HKLM "Software\qvq9999\disk-manager" "CleanOthers"
!macroend
