with open("conservatory/src/theme.rs", "r") as f:
    lines = f.readlines()

new_lines = []
in_tests = False
for line in lines:
    if line.startswith("#[cfg(test)]"):
        in_tests = True
    
    if in_tests:
        if line == "}\n" and prev_line == "    }\n":
            in_tests = False
            continue
        prev_line = line
        continue

    # Remove the standard palette lines
    if "pub const BG_WINDOW:" in line or "pub const BG_VIEW:" in line or \
       "pub const BG_HEADER:" in line or "pub const BG_CARD:" in line or \
       "pub const FG:" in line or "pub const FG_DIM:" in line or \
       "pub const GRID:" in line or "pub const ACCENT:" in line or \
       "pub const ON_ACCENT:" in line or "pub const WARN:" in line or \
       "pub const BG_RAISED:" in line or \
       "pub const ERR:" in line or "pub const OK:" in line:
        continue
    
    # Replace sheet()
    if line.startswith("pub fn sheet() -> String {"):
        new_lines.append("""
pub fn sheet() -> String {
    let mut p = vir_gtk::theme::Palette::dragon();
    p.accent = "#c4746e"; // dragonRed
    p.on_accent = "#12120f";
    p.replace_tokens(TEMPLATE)
}

pub fn install() {
    vir_gtk::theme::install_stylesheet(&sheet());
}
""")
        break # Skip the rest since it's tests and install()
    
    new_lines.append(line)
    prev_line = line

with open("conservatory/src/theme.rs", "w") as f:
    f.writelines(new_lines)
