//! "Open in Roxaruu" on a folder's right-click menu in the file manager.
//! Windows' is the installer's. On macOS and Linux an installed copy writes
//! its own as it starts, pointing at itself: so it follows the app wherever
//! it is (and whatever an update put there), and a copy built from source
//! (or a test) writes nothing.
//!
//! - macOS: Finder's Quick Actions, a workflow in `~/Library/Services`.
//! - Linux: Dolphin's and Nemo's menus. Nautilus has only its Scripts menu,
//!   which shows on files too, so it gets none: folders only.
//!
//! Files only, never a folder's: nothing here shows on a file.

use std::path::{Path, PathBuf};

/// What the menu item says.
const TITLE: &str = "Open in Roxaruu";

/// One file to write: where, what, and whether it's run (KDE 6 wants its
/// menus executable).
#[derive(Debug, PartialEq)]
struct File {
    path: PathBuf,
    text: String,
    executable: bool,
}

/// Write the menu items for this copy, if it's an installed one; a file
/// already right is left alone.
pub fn install() {
    let Ok(exe) = std::env::current_exe().and_then(dunce::canonicalize) else { return };
    let (Some(home), Some(data)) = (dirs::home_dir(), dirs::data_dir()) else { return };
    let files = files(std::env::consts::OS, &home, &data, &exe);
    let mut wrote = false;
    for f in &files {
        if std::fs::read_to_string(&f.path).is_ok_and(|t| t == f.text) {
            continue;
        }
        let written = f.path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&f.path, &f.text));
        match written {
            Ok(()) => {
                wrote = true;
                #[cfg(unix)]
                if f.executable {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&f.path, std::fs::Permissions::from_mode(0o755));
                }
                tracing::info!(path = %f.path.display(), "wrote the file manager's Open in Roxaruu");
            }
            Err(e) => tracing::warn!(path = %f.path.display(), error = %e, "couldn't write Open in Roxaruu"),
        }
    }
    // Finder's menu picks up a new Quick Action on the next look.
    if wrote && cfg!(target_os = "macos") {
        let _ = std::process::Command::new("/System/Library/CoreServices/pbs").arg("-update").status();
    }
}

/// The menu items for a copy at `exe` on `os`, for the user with `home` and
/// data folder `data` (Linux's `~/.local/share`). None for a copy that
/// isn't installed (built from source, unzipped).
fn files(os: &str, home: &Path, data: &Path, exe: &Path) -> Vec<File> {
    let file = |path: PathBuf, text: String, executable| File { path, text, executable };
    match os {
        // …/Roxaruu.app/Contents/MacOS/roxaruu
        "macos" if exe.ancestors().nth(3).is_some_and(|app| app.extension().is_some_and(|e| e == "app")) => {
            let at = home.join("Library/Services").join(format!("{TITLE}.workflow/Contents"));
            vec![
                file(at.join("Info.plist"), workflow_info(), false),
                file(at.join("document.wflow"), workflow(exe), false),
            ]
        }
        "linux" if matches!(crate::update::install_of(os, exe), crate::update::Install::LinuxDeb | crate::update::Install::LinuxTar(_)) => {
            let dolphin = dolphin(exe);
            vec![
                // KDE 6, then 5.
                file(data.join("kio/servicemenus/roxaruu.desktop"), dolphin.clone(), true),
                file(data.join("kservices5/ServiceMenus/roxaruu.desktop"), dolphin, false),
                file(data.join("nemo/actions/roxaruu.nemo_action"), nemo(exe), false),
            ]
        }
        _ => Vec::new(),
    }
}

/// `exe` quoted for a .desktop file's `Exec`.
fn desktop_exec(exe: &Path) -> String {
    let mut q = String::from("\"");
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            q.push('\\');
        }
        q.push(c);
    }
    q.push('"');
    q
}

fn dolphin(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Service\nServiceTypes=KonqPopupMenu/Plugin\nMimeType=inode/directory;\n\
         Actions=open;\nX-KDE-Priority=TopLevel\n\n\
         [Desktop Action open]\nName={TITLE}\nIcon=roxaruu\nExec={} %f\n",
        desktop_exec(exe)
    )
}

fn nemo(exe: &Path) -> String {
    format!(
        "[Nemo Action]\nName={TITLE}\nComment=Open this folder in Roxaruu\nExec={} %F\n\
         Icon-Name=roxaruu\nSelection=s\nExtensions=dir;\n",
        desktop_exec(exe)
    )
}

/// The Quick Action's own Info.plist: Finder, folders only.
fn workflow_info() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{TITLE}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.folder</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

/// The Quick Action: one Run Shell Script step, `exe <folder>` for each
/// folder chosen (an open one comes to the front, as with `rox`).
fn workflow(exe: &Path) -> String {
    let quoted = format!("'{}'", exe.to_string_lossy().replace('\'', r"'\''"));
    let script = xml(&format!("for f in \"$@\"; do {quoted} \"$f\"; done"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.path</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{script}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/sh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>5B4E6E54-0C2E-4C38-9A33-1F3E4C1A0001</string>
				<key>OutputUUID</key>
				<string>5B4E6E54-0C2E-4C38-9A33-1F3E4C1A0002</string>
				<key>UUID</key>
				<string>5B4E6E54-0C2E-4C38-9A33-1F3E4C1A0003</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>isViewVisible</key>
				<integer>1</integer>
			</dict>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleIDsByPath</key>
		<dict/>
		<key>applicationPaths</key>
		<array/>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.folder</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<false/>
		<key>serviceApplicationBundleID</key>
		<string>com.apple.finder</string>
		<key>serviceApplicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.folder</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<false/>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<false/>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#
    )
}

/// Text inside an XML element.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_installed_copy_gets_the_menu() {
        let (home, data) = (Path::new("/Users/me"), Path::new("/Users/me/.local/share"));
        let mac = files("macos", home, data, Path::new("/Applications/Roxaruu.app/Contents/MacOS/roxaruu"));
        let paths: Vec<_> = mac.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            [
                home.join("Library/Services/Open in Roxaruu.workflow/Contents/Info.plist"),
                home.join("Library/Services/Open in Roxaruu.workflow/Contents/document.wflow")
            ]
        );
        assert!(files("macos", home, data, Path::new("/Users/me/roxaruu/target/release/roxaruu")).is_empty());
        assert!(files("linux", home, data, Path::new("/home/me/src/roxaruu/target/release/roxaruu")).is_empty());
        assert!(files("windows", home, data, Path::new(r"C:\Roxaruu\roxaruu.exe")).is_empty(), "the installer's");

        let deb = files("linux", home, data, Path::new("/usr/bin/roxaruu"));
        let names: Vec<_> = deb.iter().map(|f| f.path.strip_prefix(data).unwrap().to_path_buf()).collect();
        assert_eq!(names, [Path::new("kio/servicemenus/roxaruu.desktop"), Path::new("kservices5/ServiceMenus/roxaruu.desktop"), Path::new("nemo/actions/roxaruu.nemo_action")]);
        assert!(deb[0].executable, "KDE 6 runs only executable menus");
    }

    #[test]
    fn folders_only_with_this_copy_quoted() {
        let exe = Path::new("/opt/my apps/rox$1/roxaruu");
        let d = dolphin(exe);
        assert!(d.contains("MimeType=inode/directory;\n") && d.contains(r#"Exec="/opt/my apps/rox\$1/roxaruu" %f"#), "{d}");
        let n = nemo(exe);
        assert!(n.contains("Extensions=dir;\n") && n.contains("Selection=s\n"), "{n}");
        assert!(workflow_info().contains("<string>public.folder</string>"));
        let w = workflow(Path::new("/Apps/It's & <me>.app/Contents/MacOS/roxaruu"));
        assert!(w.contains(r#"for f in "$@"; do '/Apps/It'\''s &amp; &lt;me&gt;.app/Contents/MacOS/roxaruu' "$f"; done"#), "{w}");
    }

    /// The workflow runs: Automator's own runner (no window) hands it a
    /// folder, and the program it names gets that folder.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_quick_action_runs_the_app_with_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (out, fake) = (dir.path().join("out"), dir.path().join("fake app"));
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$1\" > '{}'\n", out.display())).unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
        let wf = dir.path().join("t.workflow/Contents");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join("Info.plist"), workflow_info()).unwrap();
        std::fs::write(wf.join("document.wflow"), workflow(&fake)).unwrap();
        let folder = dir.path().join("my project");
        std::fs::create_dir(&folder).unwrap();
        for f in ["Info.plist", "document.wflow"] {
            let lint = std::process::Command::new("plutil").arg("-lint").arg(wf.join(f)).output().unwrap();
            assert!(lint.status.success(), "{f}: {}", String::from_utf8_lossy(&lint.stdout));
        }
        let ran = std::process::Command::new("automator").arg("-i").arg(&folder).arg(dir.path().join("t.workflow")).output().unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), folder.to_string_lossy());
    }
}
