pub mod history_tab;
pub mod proxy_tab;
pub mod repeater_tab;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Proxy,
    History,
    Repeater,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Proxy, Tab::History, Tab::Repeater];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Proxy => "Proxy",
            Tab::History => "History",
            Tab::Repeater => "Repeater",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::Proxy => Tab::History,
            Tab::History => Tab::Repeater,
            Tab::Repeater => Tab::Proxy,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Proxy => Tab::Repeater,
            Tab::History => Tab::Proxy,
            Tab::Repeater => Tab::History,
        }
    }
}
