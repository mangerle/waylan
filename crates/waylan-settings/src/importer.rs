//! 外部词库与字典数据导入前端适配模块
//!
//! # 设计原理
//! - **实现初衷**：为设置界面提供用户外部拼音词库、英文词库及特殊符号表的增量导入入口。
//! - **核心优势**：
//!   1. 彻底将词库校验、解析与合入逻辑下沉委托至 `waylan_engine::importer::DictImportService`；
//!   2. 直接对接并复用 `waylan-engine` 导出的内聚领域强类型错误 `DictImportError`，消除跨层冗余错误定义与手工映射代码；
//!   3. 零技术债与逻辑复制，确保 CLI 与 GUI 导入行为 100% 严格一致；
//!   4. 前端调用点已通过 `tasks.rs` 在独立后台工作线程（如 `pinyin-importer`）中异步执行，
//!      配合 `TaskRunningGuard` 杜绝重入，彻底避免百 MB 大词库解析阻塞 Slint GUI 事件循环。
//! - **代价与局限**：
//!   模块依赖 `waylan-engine` 提供的错误模型契约。

use std::path::Path;
pub use waylan_core::MAX_IMPORT_FILE_BYTES;
use waylan_core::config::AppConfig;
use waylan_engine::AuxiliaryDict;
pub use waylan_engine::importer::DictImportError as ImportError;
use waylan_engine::importer::DictImportService;

/// 解析并合入外部拼音词库
///
/// # Errors
/// 当文件不存在、体积超限、解析格式错误或写入磁盘失败时返回 [`ImportError`]。
pub fn import_pinyin_dict_file(path_str: &str) -> Result<String, ImportError> {
    let path = Path::new(path_str.trim());
    let user_dict_path = AppConfig::user_dict_path();
    let report = DictImportService::import_pinyin_dict(path, &user_dict_path)?;

    Ok(format!(
        "成功合入 {} 条词目，当前自适应词库总词量: {} 条",
        report.imported_count, report.total_entries
    ))
}

/// 解析并合入外部英文单词库
///
/// # Errors
/// 当文件不存在、体积超限、读取失败或写入磁盘失败时返回 [`ImportError`]。
pub fn import_english_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), ImportError> {
    let path = Path::new(path_str.trim());
    let aux_path = AppConfig::user_aux_dict_path();
    let report = DictImportService::import_english_dict(path, aux, Some(&aux_path))?;

    Ok((
        format!(
            "成功导入 {} 个英文单词，当前单词库总数: {}",
            report.added_count, report.total_count
        ),
        report.total_count,
    ))
}

/// 解析并合入外部符号映射表
///
/// # Errors
/// 当文件不存在、体积超限、读取失败或写入磁盘失败时返回 [`ImportError`]。
pub fn import_symbols_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), ImportError> {
    let path = Path::new(path_str.trim());
    let aux_path = AppConfig::user_aux_dict_path();
    let report = DictImportService::import_symbols_dict(path, aux, Some(&aux_path))?;

    Ok((
        format!(
            "成功导入 {} 组符号联想项，当前符号总数: {}",
            report.added_count, report.total_count
        ),
        report.total_count,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_import_nonexistent_file() {
        let res = import_pinyin_dict_file("/path/not/exist/foo.txt");
        assert!(matches!(res, Err(ImportError::FileNotFound { .. })));

        let mut aux = AuxiliaryDict::default();
        let res_en = import_english_dict_file("/path/not/exist/en.txt", &mut aux);
        assert!(matches!(res_en, Err(ImportError::FileNotFound { .. })));

        let res_sym = import_symbols_dict_file("/path/not/exist/sym.txt", &mut aux);
        assert!(matches!(res_sym, Err(ImportError::FileNotFound { .. })));
    }

    #[test]
    fn test_import_english_and_symbols_flow() {
        let mut aux = AuxiliaryDict::default();

        let temp_en = NamedTempFile::new().unwrap();
        fs::write(
            temp_en.path(),
            "---\nname: english\n...\nhello\t100\nworld\t200\nhello\t300\n# comment\n",
        )
        .unwrap();

        let (msg, count) =
            import_english_dict_file(&temp_en.path().to_string_lossy(), &mut aux).unwrap();
        assert_eq!(count, 2);
        assert!(msg.contains("成功导入 2 个英文单词"));

        let temp_sym = NamedTempFile::new().unwrap();
        fs::write(
            temp_sym.path(),
            "sj\t(｡･ω･｡)\t颜文字\nsj\t(｡･ω･｡)\t重复项\nhh\t哈哈\n",
        )
        .unwrap();

        let (msg_sym, count_sym) =
            import_symbols_dict_file(&temp_sym.path().to_string_lossy(), &mut aux).unwrap();
        assert_eq!(count_sym, 2);
        assert!(msg_sym.contains("成功导入 2 组符号联想项"));
    }

    #[test]
    fn test_import_unsupported_format() {
        let temp_doc = NamedTempFile::new().unwrap();
        let doc_path = temp_doc.path().with_extension("unsupported_xyz");
        fs::write(&doc_path, b"test").unwrap();

        let res = import_pinyin_dict_file(&doc_path.to_string_lossy());
        assert!(matches!(res, Err(ImportError::UnsupportedFormat { .. })));

        let _ = fs::remove_file(doc_path);
    }
}
