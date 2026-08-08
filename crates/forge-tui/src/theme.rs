use ratatui::style::Color;

/// Central theme tokens. Every color in the TUI comes from here.
///
/// A second theme is just another `Theme` instance swapped into
/// `THEME` (e.g. dark/light, or per-user config later).
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Terminal background (content area).
    pub background: Color,
    /// Panel / card backgrounds (picker, help, todo).
    pub surface: Color,
    /// Raised surface, e.g. highlighted list rows.
    pub surface_alt: Color,
    /// Default (unfocused) borders.
    pub border: Color,
    /// Focused border.
    pub border_focus: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary / muted text (status hints, timestamps).
    pub text_muted: Color,
    /// Accent for emphasis (headers, focus, prompts).
    pub text_accent: Color,
    /// Text selection highlight.
    pub selection_bg: Color,
    pub selection_fg: Color,
    /// Semantic colors.
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    /// Secondary accent (parallel tool output, misc emphasis).
    pub accent_alt: Color,
    /// Text color placed on top of an accent-colored background (picker selection).
    pub on_accent: Color,
    /// Code-fence body background and tokens.
    pub code_bg: Color,
    pub code_fg: Color,
    pub code_string: Color,
    pub code_comment: Color,
    pub code_number: Color,
    /// Thought-block and tool-card backgrounds.
    pub thought_bg: Color,
    pub tool_bg: Color,
    /// Overlay header background.
    pub header_bg: Color,
}

/// Default theme — matches the classic look.
pub static THEME: Theme = Theme {
    background: Color::Black,
    surface: Color::Black,
    surface_alt: Color::Rgb(40, 40, 60),
    border: Color::DarkGray,
    border_focus: Color::Cyan,
    text: Color::White,
    text_muted: Color::DarkGray,
    text_accent: Color::Cyan,
    selection_bg: Color::Rgb(50, 75, 110),
    selection_fg: Color::White,
    success: Color::Green,
    warning: Color::Yellow,
    error: Color::Red,
    info: Color::Blue,
    accent_alt: Color::Magenta,
    on_accent: Color::Black,
    code_bg: Color::Rgb(28, 28, 36),
    code_fg: Color::LightYellow,
    code_string: Color::Green,
    code_comment: Color::DarkGray,
    code_number: Color::Cyan,
    thought_bg: Color::Rgb(25, 25, 32),
    tool_bg: Color::Rgb(22, 22, 28),
    header_bg: Color::Rgb(30, 30, 40),
};
