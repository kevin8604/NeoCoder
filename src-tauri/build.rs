fn main() {
    tauri_build::build();

    // ── 测试二进制 manifest ──────────────────────────────────────────────
    // cargo test 生成的测试 exe 没有嵌入 manifest,导致 comctl32 回退到
    // System32 的 v5.82,缺失 TaskDialogIndirect 入口点,测试进程启动即崩溃
    // (STATUS_ENTRYPOINT_NOT_FOUND / 0xc0000139)。为主程序构建的 tauri
    // 注：cargo 的 rustc-link-arg-tests 不作用于 lib 单元测试目标（cargo
    // issue #10937 仍未修复），只能用全局 rustc-link-arg；主程序 manifest 由
    // tauri 通过 embed-resource 嵌入，再用 /MANIFEST:EMBED 会报资源重复
    // CVT1100，因此对 bin 目标显式 /MANIFEST:NO 覆盖（保留 tauri 的 res）。
    #[cfg(target_os = "windows")]
    {
        let manifest =
            std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
                .join("neecoder_test.manifest");
        std::fs::write(
            &manifest,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#,
        )
        .expect("write test manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
            manifest.display()
        );
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    }
}
