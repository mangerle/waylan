//! 微澜偏好设置中心 Slint 构建脚本

fn main() {
    slint_build::compile("ui/settings.slint").expect("编译 Slint UI 模板失败");
}
