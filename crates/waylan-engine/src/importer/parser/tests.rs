//! 外部词库多格式解析器单元测试

use super::DictImporter;
use super::scel;

#[test]
fn test_parse_rime_yaml() {
    let sample = "---\nname: sample\nversion: \"1.0\"\n...\n汉字\than'zi\t500\n微澜\twei'lan\n";
    let res = DictImporter::parse_rime_yaml(sample).unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].text, "汉字");
    assert_eq!(res[0].pinyin, "han'zi");
    assert_eq!(res[0].weight, 500);
    assert_eq!(res[1].text, "微澜");
    assert_eq!(res[1].pinyin, "wei'lan");

    let headerless = "长安\tCháng''ān\t800\n";
    let res2 = DictImporter::parse_rime_yaml(headerless).unwrap();
    assert_eq!(res2.len(), 1);
    assert_eq!(res2[0].text, "长安");
    assert_eq!(res2[0].pinyin, "chang'an");
}

#[test]
fn test_parse_plain_text() {
    let sample = "测试 ce'shi 100\n输入法 shū''rù'fǎ 200\n";
    let res = DictImporter::parse_plain_text(sample).unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].text, "测试");
    assert_eq!(res[1].text, "输入法");
    assert_eq!(res[1].pinyin, "shu'ru'fa");
}

#[test]
fn test_parse_sogou_scel_valid_and_corrupted_bounds() {
    let tiny_data = vec![0u8; 100];
    assert!(DictImporter::parse_sogou_scel(&tiny_data).is_err());

    let mut scel_data = vec![0u8; 0x2628];
    // 标准文件头魔数与拼音表魔数必须同时匹配
    scel_data[0..4].copy_from_slice(&[0x40, 0x15, 0x00, 0x00]);
    let mut py_cursor = 0x1540;
    scel_data[py_cursor..py_cursor + 4].copy_from_slice(&[0x9D, 0x01, 0x00, 0x00]);
    py_cursor += 4;

    scel_data[py_cursor..py_cursor + 2].copy_from_slice(&0u16.to_le_bytes());
    let wei_u16: Vec<u8> = "wei".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    scel_data[py_cursor + 2..py_cursor + 4].copy_from_slice(&(wei_u16.len() as u16).to_le_bytes());
    scel_data[py_cursor + 4..py_cursor + 4 + wei_u16.len()].copy_from_slice(&wei_u16);
    py_cursor += 4 + wei_u16.len();

    scel_data[py_cursor..py_cursor + 2].copy_from_slice(&1u16.to_le_bytes());
    let lan_u16: Vec<u8> = "lan".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    scel_data[py_cursor + 2..py_cursor + 4].copy_from_slice(&(lan_u16.len() as u16).to_le_bytes());
    scel_data[py_cursor + 4..py_cursor + 4 + lan_u16.len()].copy_from_slice(&lan_u16);

    // 缺少文件头魔数但仅有拼音区魔数也应被拒绝
    let mut bad_header = scel_data.clone();
    bad_header[0..4].fill(0);
    assert!(DictImporter::parse_sogou_scel(&bad_header).is_err());

    scel_data.extend_from_slice(&1u16.to_le_bytes());
    scel_data.extend_from_slice(&4u16.to_le_bytes());
    scel_data.extend_from_slice(&0u16.to_le_bytes());
    scel_data.extend_from_slice(&1u16.to_le_bytes());

    let word_u16: Vec<u8> = "微澜"
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    scel_data.extend_from_slice(&(word_u16.len() as u16).to_le_bytes());
    scel_data.extend_from_slice(&word_u16);
    scel_data.extend_from_slice(&4u16.to_le_bytes());
    scel_data.extend_from_slice(&[1, 2, 3, 4]);

    // 尾部畸变词条组（ext_len 故意越界）
    scel_data.extend_from_slice(&1u16.to_le_bytes());
    scel_data.extend_from_slice(&2u16.to_le_bytes());
    scel_data.extend_from_slice(&0u16.to_le_bytes());
    scel_data.extend_from_slice(&(word_u16.len() as u16).to_le_bytes());
    scel_data.extend_from_slice(&word_u16);
    scel_data.extend_from_slice(&0xFFFFu16.to_le_bytes());

    let parsed = DictImporter::parse_sogou_scel(&scel_data).expect("解析含尾部畸变的SCEL不应崩溃");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].text, "微澜");
    assert_eq!(parsed[0].pinyin, "wei'lan");

    // 验证奇数字节长度 UTF-16LE 被严格拒绝
    assert!(scel::parse_sogou_scel(&bad_header).is_err());
}
