pub mod history_tab;
pub mod proxy_tab;
pub mod repeater_tab;
pub mod rules_tab;
pub mod tools_tab;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Proxy,
    History,
    Repeater,
    Rules,
    Tools,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Proxy, Tab::History, Tab::Repeater, Tab::Rules, Tab::Tools];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Proxy => "Proxy",
            Tab::History => "History",
            Tab::Repeater => "Repeater",
            Tab::Rules => "Rules",
            Tab::Tools => "Tools",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::Proxy => Tab::History,
            Tab::History => Tab::Repeater,
            Tab::Repeater => Tab::Rules,
            Tab::Rules => Tab::Tools,
            Tab::Tools => Tab::Proxy,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Proxy => Tab::Tools,
            Tab::History => Tab::Proxy,
            Tab::Repeater => Tab::History,
            Tab::Rules => Tab::Repeater,
            Tab::Tools => Tab::Rules,
        }
    }
}
