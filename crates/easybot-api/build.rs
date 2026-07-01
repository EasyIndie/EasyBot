//! 构建脚本：自动生成文档页及前端模板
//!
//! 扫描项目根目录 `docs/` 中的 `.md` 文件，按文件名前缀数字排序，
//! 使用 pulldown-cmark 转为 HTML，注入到 `docs_layout.html` 模板中，
//! 输出 `templates/gen/docs.html`。
//!
//! 同时处理 admin.html（admin_layout.html + js + css）和 home.html（home_layout.html）。
//!
//! 所有生成产物统一输出到 `templates/gen/` 目录，该目录已在 .gitignore 中排除。
//!
//! 新增/修改/删除 `docs/` 下的 `.md` 文件后，重新运行 `cargo build` 即可自动更新。

use base64::Engine;
use pulldown_cmark::{Options, Parser, html};
use std::path::Path;

fn main() {
    // Cargo 对 build.rs 的设置：CARGO_MANIFEST_DIR = 包根目录（crates/easybot-api/）
    let manifest_dir_str = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_dir = Path::new(&manifest_dir_str);
    let gen_dir = manifest_dir.join("templates/gen");
    let docs_dir = manifest_dir.join("../../docs");
    let template_path = manifest_dir.join("templates/docs_layout.html");
    let output_path = gen_dir.join("docs.html");
    let hljs_js_path = manifest_dir.join("templates/vendor/highlight.min.js");
    let hljs_css_path = manifest_dir.join("templates/vendor/atom-one-dark.min.css");

    // 确保 gen/ 输出目录存在
    std::fs::create_dir_all(&gen_dir).ok();

    // 告知 Cargo 在内容变更时重新运行 build.rs
    println!("cargo::rerun-if-changed={}", docs_dir.display());
    println!("cargo::rerun-if-changed={}", template_path.display());
    println!("cargo::rerun-if-changed={}", hljs_js_path.display());
    println!("cargo::rerun-if-changed={}", hljs_css_path.display());

    // 读取 highlight.js vendor 文件
    let hljs_js = if hljs_js_path.exists() {
        std::fs::read_to_string(&hljs_js_path).unwrap_or_default()
    } else {
        String::new()
    };
    let hljs_css = if hljs_css_path.exists() {
        std::fs::read_to_string(&hljs_css_path).unwrap_or_default()
    } else {
        String::new()
    };

    // 收集并排序 .md 文件
    let docs_dir = docs_dir.canonicalize().unwrap_or(docs_dir);
    if !docs_dir.exists() {
        // docs 目录不存在时生成一个占位页面
        let fallback = generate_fallback(&template_path);
        std::fs::write(&output_path, fallback).unwrap();
        return;
    }

    let mut entries: Vec<_> = std::fs::read_dir(&docs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    if entries.is_empty() {
        let fallback = generate_fallback(&template_path);
        std::fs::write(&output_path, fallback).unwrap();
        return;
    }

    entries.sort_by_key(|e| e.file_name());

    // 解析每个文件：提取标题 + 转 HTML
    let mut sidebar_items = String::new();
    let mut doc_sections = String::new();

    for entry in &entries {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).unwrap();

        let title = extract_title(&content, &file_name);
        // 用文件名（去掉 .md）作为锚点 ID
        let id = file_name.trim_end_matches(".md");
        let html_content = md_to_html(&content);

        sidebar_items.push_str(&format!("<a href=\"#{}\">{}</a>\n", id, title));
        doc_sections.push_str(&format!(
            "<section id=\"{}\"><h2>{}</h2>{}</section>\n",
            id, title, html_content
        ));
    }

    // 注入模板
    let template = if template_path.exists() {
        std::fs::read_to_string(&template_path).unwrap()
    } else {
        generate_default_template()
    };

    let result = process_includes(&template, manifest_dir)
        .replace("__SIDEBAR_ITEMS__", &sidebar_items)
        .replace("__DOCS_CONTENT__", &doc_sections)
        .replace("__HLJS_CSS__", &hljs_css)
        .replace("__HLJS_JS__", &hljs_js);

    std::fs::write(&output_path, &result).unwrap();

    // ── 读取 favicon/logo 并 base64 编码 ──
    let favicon_path = manifest_dir.join("templates/favicon.png");
    let logo_path = manifest_dir.join("templates/logo.png");
    let logo_small_path = manifest_dir.join("templates/logo-small.png");

    println!("cargo::rerun-if-changed={}", favicon_path.display());
    println!("cargo::rerun-if-changed={}", logo_path.display());
    println!("cargo::rerun-if-changed={}", logo_small_path.display());

    let favicon_data = png_to_data_uri(&favicon_path);
    let logo_data = png_to_data_uri(&logo_path);
    let logo_small_data = png_to_data_uri(&logo_small_path);

    // ── 首页：从 home_layout.html 生成 home.html（注入 favicon/logo） ──
    let home_layout_path = manifest_dir.join("templates/home_layout.html");
    let home_output_path = gen_dir.join("home.html");
    println!("cargo::rerun-if-changed={}", home_layout_path.display());
    if home_layout_path.exists() {
        let home_html = std::fs::read_to_string(&home_layout_path).unwrap();
        let home_html = home_html
            .replace("__FAVICON__", &favicon_data)
            .replace("__LOGO__", &logo_data);
        std::fs::write(&home_output_path, home_html).unwrap();
    }

    // ── 管理后台：拼接 JS/CSS 模块生成 admin.html ──
    let admin_layout_path = manifest_dir.join("templates/admin_layout.html");
    let admin_output_path = gen_dir.join("admin.html");
    let js_dir = manifest_dir.join("templates/js");
    let css_dir = manifest_dir.join("templates/css");

    println!("cargo::rerun-if-changed={}", admin_layout_path.display());
    println!("cargo::rerun-if-changed={}", js_dir.display());
    println!("cargo::rerun-if-changed={}", css_dir.display());

    let admin_css = std::fs::read_to_string(css_dir.join("admin.css")).unwrap_or_default();

    // 读取 JS（单文件包含所有逻辑）
    let admin_js = std::fs::read_to_string(js_dir.join("admin.js")).unwrap_or_default();

    if admin_layout_path.exists() {
        let layout = std::fs::read_to_string(&admin_layout_path).unwrap();
        let layout = process_includes(&layout, manifest_dir);
        let admin_html = layout
            .replace("__ADMIN_CSS__", &admin_css)
            .replace("__ADMIN_JS__", &admin_js)
            .replace("__FAVICON__", &favicon_data)
            .replace("__LOGO_SMALL__", &logo_small_data)
            .replace("__LOGO__", &logo_data);
        std::fs::write(&admin_output_path, admin_html).unwrap();
    }

    // ── 文档页：注入 favicon ──
    if output_path.exists() {
        let docs_html = std::fs::read_to_string(&output_path).unwrap();
        let docs_html = docs_html.replace("__FAVICON__", &favicon_data);
        std::fs::write(&output_path, docs_html).unwrap();
    }

    // 构建完成，不输出额外消息避免 cargo:warning 干扰
}

/// 读取 PNG 文件并返回 base64 data URI
fn png_to_data_uri(path: &Path) -> String {
    if !path.exists() {
        return String::new();
    }
    let data = std::fs::read(path).unwrap_or_default();
    if data.is_empty() {
        return String::new();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    format!("data:image/png;base64,{}", b64)
}

/// 提取 Markdown 文件的标题
fn extract_title(content: &str, file_name: &str) -> String {
    // 取第一个 # 标题
    if let Some(line) = content.lines().next()
        && let Some(title) = line.strip_prefix("# ")
    {
        return title.to_string();
    }
    // 无标题时从文件名生成：去掉数字前缀和 .md
    file_name
        .trim_end_matches(".md")
        .split('-')
        .skip(1)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// 处理 {% include "filename" %} 模板指令
fn process_includes(template: &str, base_dir: &Path) -> String {
    let re = regex::Regex::new(r#"\{%\s*include\s*"([^"]+)"\s*%\}"#).unwrap();
    re.replace_all(template, |caps: &regex::Captures| {
        let filename = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let path = base_dir.join(filename);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|_| format!("<!-- include not found: {} -->", filename))
    })
    .to_string()
}

/// 将 Markdown 转为 HTML（启用 GFM 表格等扩展）
fn md_to_html(md: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(md, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// 生成默认模板（当模板文件不存在时的后备方案）
fn generate_default_template() -> String {
    r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="UTF-8"><title>EasyBot 文档</title>
<style>
body { background:#0f0f1a; color:#c9d1d9; font-family:system-ui,sans-serif; margin:0; }
.header { background:#161822; padding:12px 24px; border-bottom:1px solid #30363d; }
.header h1 { color:#58a6ff; font-size:18px; margin:0; }
.layout { display:flex; }
.sidebar { width:240px; background:#161822; border-right:1px solid #30363d; padding:16px 0; }
.sidebar a { display:block; padding:8px 20px; color:#8b949e; text-decoration:none; font-size:14px; border-left:3px solid transparent; }
.sidebar a:hover { color:#c9d1d9; background:#1c1e2e; }
.content { flex:1; padding:32px 48px; max-width:900px; }
.content h2 { color:#e6edf3; border-bottom:1px solid #21262d; padding-bottom:8px; }
.content h3 { color:#e6edf3; }
.content p { line-height:1.7; }
.content code { background:#1c1e2e; padding:2px 6px; border-radius:4px; font-family:monospace; }
.content pre { background:#161822; border:1px solid #30363d; border-radius:8px; padding:16px; overflow-x:auto; }
.content table { width:100%; border-collapse:collapse; }
.content table th, td { border:1px solid #30363d; padding:8px 12px; text-align:left; }
.content table th { background:#161822; }
.content a { color:#58a6ff; }
@media (max-width:768px) { .layout { flex-direction:column; } .sidebar { width:100%; } }
</style></head>
<body>
<div class="header"><h1>EasyBot 文档</h1></div>
<div class="layout">
  <nav class="sidebar">__SIDEBAR_ITEMS__</nav>
  <main class="content">__DOCS_CONTENT__</main>
</div>
</body></html>
"#.to_string()
}

/// 生成占位页面（docs 不存在或为空时）
fn generate_fallback(template_path: &Path) -> String {
    let template = if template_path.exists() {
        std::fs::read_to_string(template_path).unwrap()
    } else {
        generate_default_template()
    };

    template
        .replace("__SIDEBAR_ITEMS__", "<a href=\"#placeholder\">概述</a>\n")
        .replace(
            "__DOCS_CONTENT__",
            "<section id=\"placeholder\"><h2>文档目录为空</h2>\
             <p>请将 Markdown 文档放入项目根目录的 <code>docs/</code> 文件夹中。</p>\
             <p>文件命名约定：<code>01-章节名.md</code>、<code>02-章节名.md</code> ...</p>\
             <p>重新编译后文档页将自动生成。</p></section>\n",
        )
}
