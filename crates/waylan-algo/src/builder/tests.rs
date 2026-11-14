use super::*;

use crate::user_adaptive::UserAdaptiveModel;
use std::fs::File;
use std::io::Write;

#[test]
fn test_build_and_import() {
    let temp_dir = std::env::temp_dir();
    let txt_path = temp_dir.join("test_dict.txt");
    let bin_path = temp_dir.join("test_dict.bin");

    // 写入测试源数据
    let mut file = File::create(&txt_path).expect("创建临时文件");
    writeln!(file, "# 注释行").unwrap();
    writeln!(file, "微澜\twei lan\t15000").unwrap();
    writeln!(file, "智能\tzhi neng\t12000").unwrap();
    drop(file);

    // 1. 编译二进制词库
    let stats = DictBuilder::build_binary_dict(&[&txt_path], &bin_path).expect("编译成功");
    assert_eq!(stats.total_words, 2);
    assert_eq!(stats.total_keys, 2);

    // 2. 导入到用户自适应模型
    let mut user_model = UserAdaptiveModel::new();
    let imported = DictBuilder::import_user_text_dict(&txt_path, &mut user_model, 1_700_000_000)
        .expect("导入成功");
    assert_eq!(imported, 2);

    // 验证用户模型中已包含自造词
    let words = user_model.lookup_user_words("wei'lan", 1_700_000_000);
    assert!(!words.is_empty());
    assert_eq!(words[0].text, "微澜");

    // 清理临时文件
    let _ = std::fs::remove_file(txt_path);
    let _ = std::fs::remove_file(bin_path);
}

#[test]
fn test_build_and_load_compact_mmap() {
    use crate::table::LexiconTable;

    let temp_dir = std::env::temp_dir();
    let txt_path = temp_dir.join("test_compact_dict.txt");
    let compact_bin_path = temp_dir.join("test_compact_dict.bin");

    // 写入测试源数据（含低词频四字成语与同形声母/简拼）
    let mut file = File::create(&txt_path).expect("创建临时文件");
    writeln!(file, "微澜\twei lan\t15000").unwrap();
    writeln!(file, "中国\tzhong guo\t20000").unwrap();
    writeln!(file, "中\tzhong\t9000").unwrap();
    writeln!(file, "最后\tzui hou\t12000").unwrap();
    writeln!(file, "人工智能\tren gong zhi neng\t31607").unwrap();
    writeln!(file, "人过中年\tren guo zhong nian\t504").unwrap();
    writeln!(file, "弱冠之年\truo guan zhi nian\t78").unwrap();
    drop(file);

    // 1. 编译为零拷贝紧凑二进制格式 (DAT 一体化)
    let stats = DictBuilder::build_compact_binary_dict(&[&txt_path], &compact_bin_path)
        .expect("紧凑编译成功");
    assert_eq!(stats.total_words, 7);

    // 2. 使用 LexiconTable::load_binary 加载
    let table = LexiconTable::load_binary(&compact_bin_path).expect("零拷贝紧凑词库加载成功");

    // 3. 验证全拼检索
    let cands = table.lookup("zhong'guo");
    assert!(!cands.is_empty());
    assert_eq!(cands[0].text, "中国");

    // 4. 验证简拼缩写检索 (含低频四字成语完整保留)
    let acronym_cands = table.lookup_acronym("zg");
    assert!(!acronym_cands.is_empty(), "简拼 zg 应召回中国");
    assert_eq!(acronym_cands[0].text, "中国");

    let rgzn_cands = table.lookup_acronym("rgzn");
    assert_eq!(rgzn_cands.len(), 3, "rgzn 应完整召回 3 个四字词/成语");
    assert_eq!(rgzn_cands[0].text, "人工智能");
    assert_eq!(rgzn_cands[1].text, "人过中年");
    assert_eq!(rgzn_cands[2].text, "弱冠之年");

    // 5. 验证单字声母预置索引与二字简拼互不覆盖 (如 "zh")
    let zh_acronym = table.lookup_acronym("zh");
    assert_eq!(zh_acronym[0].text, "最后");
    let zh_initial = table.get_initial_raw_entries("zh");
    assert!(zh_initial.is_some(), "翘舌声母 zh 不应被二字简拼 zh 覆盖");
    assert_eq!(zh_initial.unwrap()[0].0, "中");

    // 6. 验证 Word ID 与反查
    let compact = table.compact_mmap.as_ref().unwrap();
    let id_zg = compact.get_id("中国");
    assert!(id_zg.is_some());
    let word_zg = compact.get_word(id_zg.unwrap());
    assert_eq!(word_zg, Some("中国"));

    // 清理临时文件
    let _ = std::fs::remove_file(txt_path);
    let _ = std::fs::remove_file(compact_bin_path);
}

#[test]
fn test_parse_dict_entry_score_and_negative_filter() {
    use crate::table::LexiconTable;

    assert_eq!(
        parse_dict_entry_score(&["地标", "di biao"]),
        Some(DEFAULT_MISSING_SCORE)
    );
    assert_eq!(
        parse_dict_entry_score(&["地标", "di biao", ""]),
        Some(DEFAULT_MISSING_SCORE)
    );
    assert_eq!(
        parse_dict_entry_score(&["地标", "di biao", "abc"]),
        Some(DEFAULT_MISSING_SCORE)
    );
    assert_eq!(
        parse_dict_entry_score(&["柏香镇", "bai xiang zhen", "-1"]),
        None
    );
    assert_eq!(
        parse_dict_entry_score(&["中国", "zhong guo", "20000"]),
        Some(20000)
    );

    let temp_dir = std::env::temp_dir();
    let txt_path = temp_dir.join("test_neg_score_dict.txt");
    let bin_path = temp_dir.join("test_neg_score_dict.bin");

    let mut file = File::create(&txt_path).expect("创建临时文件");
    writeln!(file, "柏香镇\tbai xiang zhen\t-1").unwrap();
    writeln!(file, "白香镇\tbai xiang zhen\t500").unwrap();
    writeln!(file, "地标\tdi biao").unwrap();
    writeln!(file, "地标二\tdi biao\t100").unwrap();
    drop(file);

    let stats = DictBuilder::build_compact_binary_dict(&[&txt_path], &bin_path).expect("编译成功");
    assert_eq!(stats.total_words, 3, "负词频词条应当被跳过");

    let table = LexiconTable::load_binary(&bin_path).expect("加载成功");
    let bx = table.lookup("bai'xiang'zhen");
    assert_eq!(bx.len(), 1);
    assert_eq!(bx[0].text, "白香镇");

    let db = table.lookup("di'biao");
    assert_eq!(db.len(), 2);
    assert_eq!(db[0].text, "地标二", "缺词频条目应低于显式词频 100");
    assert_eq!(db[1].text, "地标");

    let _ = std::fs::remove_file(txt_path);
    let _ = std::fs::remove_file(bin_path);
}
