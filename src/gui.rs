use core::time;
use iced::{
    executor,
    widget::{
        focus_next, focus_previous, scrollable,
        text_editor::{Action, Content, Edit},
        Button, Checkbox, Column, Container, PickList, Row, Rule, Space, Text, TextEditor,
        TextInput,
    },
    Application, Command, Element, Length, Subscription, Theme,
};
use miniscript::{
    bitcoin::{Address, Amount, Denomination},
    Descriptor, DescriptorPublicKey,
};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fmt::{self, Display, Formatter},
    fs,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use crate::bitcoind::{
    self, BitcoinMessage, BitcoindListener, GenerateToAddress, GenerateToDescriptor,
    SendEveryBlock, SendToAddress, SendToDescriptor,
};

const MAX_DERIV: u32 = 2u32.pow(31) - 1;

fn bitcoind_default_cookie_path() -> String {
    #[cfg(target_os = "windows")]
    let mut path = {
        let mut path = env::var("APPDATA").map(PathBuf::from).unwrap();
        path.push("Bitcoin");
        path.push(".cookie");
        path
    };

    #[cfg(not(target_os = "windows"))]
    let path = {
        let mut path = env::var("HOME")
            .map(PathBuf::from)
            .expect("$HOME should exists");
        path.push(".bitcoin");
        path.push(".cookie");
        path
    };

    path.to_str().expect("cookie path should be ok").to_string()
}

fn config_path() -> String {
    #[cfg(target_os = "windows")]
    let mut path = {
        let mut path = env::var("APPDATA").map(PathBuf::from).unwrap();
        path.push("Minta");
        path.push("minta.conf");
        path
    };

    #[cfg(not(target_os = "windows"))]
    let path = {
        let mut path = env::var("HOME")
            .map(PathBuf::from)
            .expect("$HOME should exists");
        path.push(".minta");
        path.push("minta.conf");
        path
    };

    path.to_str().expect("path should be ok").to_string()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    pub bitcoind: BitcoindConfig,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitcoindConfig {
    pub auth_type: AuthMethod,
    pub user: String,
    pub password: String,
    pub cookie_path: String,
    pub address: String,
}

impl Default for BitcoindConfig {
    fn default() -> Self {
        Self {
            auth_type: AuthMethod::default(),
            user: "user".into(),
            password: "password".into(),
            cookie_path: bitcoind_default_cookie_path(),
            address: "127.0.0.1:18443".into(),
        }
    }
}

impl Config {
    pub fn new() -> Self {
        if let Ok(file) = fs::File::open(config_path()) {
            if let Ok(config) = serde_yaml::from_reader(file) {
                return config;
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        log::info!("save({})", config_path());
        let path = config_path();
        let p = PathBuf::from(config_path());
        let parent = p.parent().to_owned().expect("Folder should exists");
        if !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(file) = fs::File::create(path) {
            match serde_yaml::to_writer(file, self) {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Failed to write config file: {}", e)),
            }
        } else {
            Err("Failed to open config file".to_string())
        }
    }
}

#[derive(Debug, Clone)]
pub enum Key {
    Tab(bool),
}

#[derive(Debug, Clone)]
pub enum Message {
    Bitcoind(BitcoinMessage),

    // text inputs
    BitcoindAddress(String),
    User(String),
    Password(String),
    CookiePath(String),
    GenerateTarget(GenerateTarget),
    BlocksGenerate(String),
    AddressGenerate(String),
    DescriptorGenerate(String),
    DescriptorIndexGenerate(String),
    AmountSend(String),
    CountSend(String),
    AddressSend(String),
    DescriptorSend(String),
    DescriptorIndexSend(String),
    MinSend(String),
    MaxSend(String),
    BlockSend(String),
    AutoblockBlocks(String),
    AutoblockTimeframe(TimeFrame),
    ConsoleEdit(Action),

    // buttons
    SelectRpcAuth(bool),
    SelectCookie(bool),
    ConnectRpcAuth,
    ConnectCookie,
    Disconnect,
    StartAutoblock,
    StopAutoblock,
    GenerateToAddress,
    GenerateToSelf,
    GenerateToRandom,
    GenerateToDescriptor,
    SendToAddress,
    SendToDescriptor,
    ToggleEveryBlock(bool),

    KeyPressed(Key),
}

#[derive(Debug)]
pub struct Flags {
    pub receiver: async_channel::Receiver<BitcoinMessage>,
    pub sender: std::sync::mpsc::Sender<BitcoinMessage>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub enum AuthMethod {
    #[default]
    RpcAuth,
    Cookie,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenerateTarget {
    Address,
    ToSelf,
    Random,
    Descriptor,
}

impl FromStr for GenerateTarget {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "address" => Ok(GenerateTarget::Address),
            "self" => Ok(GenerateTarget::ToSelf),
            "random" => Ok(GenerateTarget::Random),
            "descriptor" => Ok(GenerateTarget::Descriptor),
            _ => Err(()),
        }
    }
}

impl Display for GenerateTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            GenerateTarget::Address => write!(f, "address"),
            GenerateTarget::ToSelf => write!(f, "self"),
            GenerateTarget::Random => write!(f, "random"),
            GenerateTarget::Descriptor => write!(f, "descriptor"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimeFrame {
    Second,
    Minute,
}

impl Display for TimeFrame {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TimeFrame::Second => write!(f, "seconds"),
            TimeFrame::Minute => write!(f, "minutes"),
        }
    }
}

impl FromStr for TimeFrame {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "seconds" => Ok(Self::Second),
            "minutes" => Ok(Self::Minute),
            _ => Err(()),
        }
    }
}

pub struct Gui {
    receiver: async_channel::Receiver<BitcoinMessage>,
    sender: std::sync::mpsc::Sender<BitcoinMessage>,
    config: Config,
    block_height: Option<u64>,
    balance: Option<Amount>,
    generate_target: GenerateTarget,
    generate_blocks: String,
    generate_address: String,
    generate_descriptor: String,
    generate_descriptor_index: String,
    send_amount: String,
    send_count: String,
    send_min: String,
    send_max: String,
    send_address: String,
    send_descriptor: String,
    send_descriptor_index: String,
    send_every_blocks: String,
    send_every_blocks_enabled: bool,
    connected: bool,
    autoblock_blocks: String,
    autoblocks_timeframe: TimeFrame,
    autoblock_wip: bool,
    generate_wip: bool,
    send_wip: bool,
    console: Content,
}

impl Gui {
    fn button(text: &str, msg: Option<Message>) -> Button<'static, Message> {
        let w = (text.len() * 10) as f32;
        let mut button = Button::new(
            Column::new()
                .push(Space::with_height(Length::Fill))
                .push(
                    Row::new()
                        .push(Space::with_width(Length::Fill))
                        .push(Text::new(text.to_string()).size(15))
                        .push(Space::with_width(Length::Fill)),
                )
                .push(Space::with_height(Length::Fill)),
        )
        .height(30)
        .width(Length::Fixed(w));
        if let Some(msg) = msg {
            button = button.on_press(msg)
        }
        button
    }

    pub fn send_to_bitcoind(&mut self, msg: BitcoinMessage) {
        if let Err(e) = self.sender.send(msg) {
            self.print(&format!("Fail to send message to bitcoind: {}", e))
        }
    }

    pub fn disconnect(&mut self) {
        self.send_to_bitcoind(BitcoinMessage::Disconnect)
    }

    pub fn connect_rpc_auth(&mut self) {
        let msg = BitcoinMessage::SetCredentials {
            address: self.config.bitcoind.address.clone(),
            auth: bitcoind::AuthMethod::RpcAuth {
                user: self.config.bitcoind.user.clone(),
                password: self.config.bitcoind.password.clone(),
            },
        };

        self.send_to_bitcoind(msg);
        self.send_to_bitcoind(BitcoinMessage::Connect);
    }

    pub fn connect_cookie(&mut self) {
        let msg = BitcoinMessage::SetCredentials {
            address: self.config.bitcoind.address.clone(),
            auth: bitcoind::AuthMethod::Cookie {
                cookie_path: self.config.bitcoind.cookie_path.clone(),
            },
        };

        self.send_to_bitcoind(msg);
        self.send_to_bitcoind(BitcoinMessage::Connect);
    }

    pub fn credentials_valid(&self) -> bool {
        match self.config.bitcoind.auth_type {
            AuthMethod::RpcAuth => {
                !self.config.bitcoind.user.is_empty()
                    && !self.config.bitcoind.password.is_empty()
                    && !self.config.bitcoind.address.is_empty()
            }
            AuthMethod::Cookie => {
                !self.config.bitcoind.cookie_path.is_empty()
                    && !self.config.bitcoind.address.is_empty()
            }
        }
    }

    pub fn generate_to_self(&mut self) {
        if let Ok(blocks) = u32::from_str(&self.generate_blocks) {
            self.send_to_bitcoind(BitcoinMessage::GenerateToSelf(blocks));
        }
    }

    pub fn generate_to_random(&mut self) {
        if let Ok(blocks) = u32::from_str(&self.generate_blocks) {
            self.send_to_bitcoind(BitcoinMessage::Generate(blocks));
        }
    }

    pub fn generate_to_address(&mut self) {
        if let (Ok(blocks), Ok(addr)) = (
            u32::from_str(&self.generate_blocks),
            Address::from_str(&self.generate_address),
        ) {
            if addr.is_valid_for_network(miniscript::bitcoin::Network::Regtest) {
                let address = addr.assume_checked();
                self.send_to_bitcoind(BitcoinMessage::GenerateToAddress(GenerateToAddress {
                    blocks,
                    address,
                }))
            } else {
                self.print("Invalid address network!")
            }
        }
    }

    pub fn generate_to_descriptor(&mut self) {
        if let (Ok(blocks), true, Ok(start_index)) = (
            u32::from_str(&self.generate_blocks),
            !self.generate_descriptor.is_empty(),
            u32::from_str(&self.generate_descriptor_index),
        ) {
            self.send_to_bitcoind(BitcoinMessage::GenerateToDescriptor(GenerateToDescriptor {
                blocks,
                descriptor: self.generate_descriptor.clone(),
                start_index,
            }));
        }
    }

    pub fn send_to_address(&mut self) {
        if let (Ok(amount), Ok(addr)) = (
            Amount::from_str_in(&self.send_amount, Denomination::Bitcoin),
            Address::from_str(&self.send_address),
        ) {
            if addr.is_valid_for_network(miniscript::bitcoin::Network::Regtest) {
                let address = addr.assume_checked();
                self.send_to_bitcoind(BitcoinMessage::SendToAddress(SendToAddress {
                    amount,
                    address,
                }))
            } else {
                self.print("Invalid address network!")
            }
        }
    }

    pub fn send_to_descriptor(&mut self) {
        if let (Ok(count), true, Ok(amount_min), Ok(amount_max), Ok(start_index)) = (
            u32::from_str(&self.send_count),
            !self.send_descriptor.is_empty(),
            Amount::from_str_in(&self.send_min, Denomination::Bitcoin),
            Amount::from_str_in(&self.send_max, Denomination::Bitcoin),
            u32::from_str(&self.send_descriptor_index),
        ) {
            self.send_to_bitcoind(BitcoinMessage::SendToDescriptor(SendToDescriptor {
                count,
                amount_min,
                amount_max,
                descriptor: self.send_descriptor.clone(),
                start_index,
            }))
        }
    }
    pub fn toggle_every_blocks(&mut self, state: bool) {
        self.send_every_blocks_enabled = state;
        let count = u32::from_str(&self.send_count);
        let min = Amount::from_str_in(&self.send_min, Denomination::Bitcoin);
        let max = Amount::from_str_in(&self.send_max, Denomination::Bitcoin);
        let descriptor =
            if Descriptor::<DescriptorPublicKey>::from_str(&self.send_descriptor).is_ok() {
                Some(self.send_descriptor.clone())
            } else {
                None
            };
        let start_index = u32::from_str(&self.send_descriptor_index);
        let every_blocks = u32::from_str(&self.send_every_blocks);
        if let (
            Ok(count),
            Ok(amount_min),
            Ok(amount_max),
            Some(descriptor),
            Ok(start_index),
            Ok(blocks),
        ) = (count, min, max, descriptor, start_index, every_blocks)
        {
            self.send_to_bitcoind(BitcoinMessage::EnableSendEveryBlock(SendEveryBlock {
                count,
                amount_min,
                amount_max,
                descriptor,
                start_index,
                blocks,
                actual_index: None,
            }))
        }
    }

    pub fn start_auto_block(&mut self) {
        log::info!("GUI.start_auto_block()");
        if !self.generate_wip && !self.send_wip && !self.autoblock_wip {
            self.autoblock_wip = true;
            let tf_ms = match self.autoblocks_timeframe {
                TimeFrame::Second => 1_000,
                TimeFrame::Minute => 60_000,
            };
            let blocks = u32::from_str(&self.autoblock_blocks).expect("input checked");
            let delay = time::Duration::from_millis((tf_ms / blocks) as u64);
            log::info!("start");
            self.send_to_bitcoind(BitcoinMessage::StartAutoBlock(delay));
        }

        if self.send_every_blocks_enabled {
            self.toggle_every_blocks(true);
        }
    }

    pub fn stop_auto_block(&mut self) {
        if self.autoblock_wip {
            self.send_to_bitcoind(BitcoinMessage::StopAutoBlock);
        }
    }

    pub fn print(&mut self, msg: &str) {
        let mut msg = msg.to_string();
        if !msg.ends_with('\n') {
            msg.push('\n');
        }

        self.console
            .perform(Action::Edit(Edit::Paste(Arc::new(msg))));
    }

    pub fn auth_panel(&self) -> Container<Message> {
        let address_input = {
            let mut input =
                TextInput::new("bitcoind address", &self.config.bitcoind.address).width(310);
            if !self.connected {
                input = input.on_input(Message::BitcoindAddress);
            }
            input
        };

        let (cookie, rpc_auth) = match (&self.config.bitcoind.auth_type, self.connected) {
            (_, true) => (false, false),
            (AuthMethod::RpcAuth, false) => (false, true),
            (AuthMethod::Cookie, false) => (true, false),
        };

        let connect = if self.connected {
            Self::button("Disconnect", Some(Message::Disconnect))
        } else {
            Self::button(
                "Connect",
                match (&self.config.bitcoind.auth_type, self.credentials_valid()) {
                    (_, false) => None,
                    (AuthMethod::RpcAuth, true) => Some(Message::ConnectRpcAuth),
                    (AuthMethod::Cookie, true) => Some(Message::ConnectCookie),
                },
            )
        };

        let mut rpc_auth_check = Checkbox::new("", rpc_auth);
        let mut cookie_check = Checkbox::new("", cookie);
        if !self.connected {
            rpc_auth_check = rpc_auth_check.on_toggle(Message::SelectRpcAuth);
            cookie_check = cookie_check.on_toggle(Message::SelectCookie);
        }

        let chain_height = self
            .block_height
            .map(|height| Text::new(format!("Block height {}", height)));

        let col = Column::new()
            .push(
                Row::new()
                    .push(Text::new("Bitcoind address: "))
                    .push(Space::with_width(Length::Fill))
                    .push(address_input),
            )
            .push(Space::with_height(10))
            .push(
                Row::new()
                    .push(rpc_auth_check)
                    .push("RpcAuth")
                    .push(Space::with_width(Length::Fill))
                    .push({
                        let mut input =
                            TextInput::new("user", &self.config.bitcoind.user).width(150);
                        if rpc_auth {
                            input = input.on_input(Message::User);
                        }
                        input
                    })
                    .push(Space::with_width(10))
                    .push({
                        let mut input =
                            TextInput::new("password", &self.config.bitcoind.password).width(150);
                        if rpc_auth {
                            input = input.on_input(Message::Password);
                        }
                        input
                    }),
            )
            .push(Space::with_height(5))
            .push(
                Row::new()
                    .push(cookie_check)
                    .push("Cookie")
                    .push(Space::with_width(Length::Fill))
                    .push({
                        let mut input =
                            TextInput::new("cookie path", &self.config.bitcoind.cookie_path)
                                .width(310);
                        if cookie {
                            input = input.on_input(Message::CookiePath);
                        }
                        input
                    }),
            )
            .push(Space::with_height(10))
            .push(
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push_maybe(if !self.connected {
                        Some(Space::with_width(Length::Fill))
                    } else {
                        None
                    })
                    .push(connect)
                    .push(Space::with_width(Length::Fill)),
            )
            .push_maybe(if chain_height.is_some() {
                Some(Space::with_height(5))
            } else {
                None
            })
            .push_maybe(if chain_height.is_some() {
                Some(Rule::horizontal(4))
            } else {
                None
            })
            .push_maybe(if chain_height.is_some() {
                Some(Space::with_height(5))
            } else {
                None
            })
            .push_maybe(chain_height);

        Container::new(col)
    }

    pub fn auto_block_panel(&self) -> Container<Message> {
        let autoblock_btn = match (self.generate_wip, self.autoblock_wip, self.connected) {
            (false, false, true) => Self::button("Generate", Some(Message::StartAutoblock)),
            (false, true, true) => Self::button("Stop", Some(Message::StopAutoblock)),
            _ => Self::button("Generate", None),
        }
        .width(100);

        let wip = self.generate_wip || self.autoblock_wip || !self.connected;

        let blocks_input = {
            let mut input = TextInput::new("blocks", &self.autoblock_blocks).width(100);
            if !wip {
                input = input.on_input(Message::AutoblockBlocks);
            }
            input
        };

        let tf_list = vec![TimeFrame::Second, TimeFrame::Minute];

        let dropdown = PickList::new(
            tf_list.clone(),
            Some(&self.autoblocks_timeframe),
            Message::AutoblockTimeframe,
        );

        let col = Row::new()
            .push(autoblock_btn)
            .push(Space::with_width(10))
            .push(blocks_input)
            .push(Text::new(" blocks every "))
            .push(dropdown)
            .align_items(iced::alignment::Alignment::Center);

        Container::new(col)
    }

    pub fn generate_panel(&self) -> Container<Message> {
        let generate_signal = match (
            &self.generate_target,
            self.generate_wip || !self.connected || self.autoblock_wip,
        ) {
            (GenerateTarget::Address, false) => Some(Message::GenerateToAddress),
            (GenerateTarget::ToSelf, false) => Some(Message::GenerateToSelf),
            (GenerateTarget::Random, false) => Some(Message::GenerateToRandom),
            (GenerateTarget::Descriptor, false) => Some(Message::GenerateToDescriptor),
            _ => None,
        };

        let generate_button = Self::button("Generate", generate_signal).width(100);

        let blocks_signal = if !self.generate_wip && self.connected {
            Some(Message::BlocksGenerate)
        } else {
            None
        };

        let blocks_input = {
            let mut input = TextInput::new("blocks", &self.generate_blocks).width(60);
            if let Some(signal) = blocks_signal {
                input = input.on_input(signal);
            }
            input
        };

        let index_input = if let GenerateTarget::Descriptor = self.generate_target {
            let mut input =
                TextInput::new("start index", &self.generate_descriptor_index).width(100);
            if !self.generate_wip && self.connected {
                input = input.on_input(Message::DescriptorIndexGenerate);
            }
            Some(input)
        } else {
            None
        };

        let address_signal = if !self.generate_wip && self.connected {
            Some(Message::AddressGenerate)
        } else {
            None
        };

        let descriptor_signal = if !self.generate_wip && self.connected {
            Some(Message::DescriptorGenerate)
        } else {
            None
        };

        let target_input = match self.generate_target {
            GenerateTarget::Address => {
                let mut input = TextInput::new("address", &self.generate_address);
                if let Some(signal) = address_signal {
                    input = input.on_input(signal);
                }
                Some(input)
            }
            GenerateTarget::Descriptor => {
                let mut input = TextInput::new("descriptor", &self.generate_descriptor);
                if let Some(signal) = descriptor_signal {
                    input = input.on_input(signal);
                }
                Some(input)
            }
            _ => None,
        };

        let target_list = vec![
            GenerateTarget::Address,
            GenerateTarget::Descriptor,
            GenerateTarget::ToSelf,
            GenerateTarget::Random,
        ];

        let targets = PickList::new(
            target_list,
            Some(&self.generate_target),
            Message::GenerateTarget,
        );

        let col = Column::new()
            .push(
                Row::new()
                    .push(generate_button)
                    .push(Space::with_width(10))
                    .push(blocks_input)
                    .push(Space::with_width(10))
                    .push(Text::new(" blocks to "))
                    .push(Space::with_width(10))
                    .push(targets)
                    .push(Space::with_width(10))
                    .push(Space::with_width(Length::Fill))
                    .align_items(iced::alignment::Alignment::Center),
            )
            .push_maybe(if target_input.is_some() {
                Some(Space::with_height(5))
            } else {
                None
            })
            .push_maybe(if target_input.is_some() {
                Some(
                    Row::new()
                        .push_maybe(target_input)
                        .push(Space::with_width(5))
                        .push_maybe(index_input),
                )
            } else {
                None
            });

        Container::new(col)
    }

    pub fn send_panel(&self) -> Container<Message> {
        let balance = self
            .balance
            .map(|balance| Text::new(format!("Balance: {}", balance)));

        let enable = !self.send_wip && self.connected;

        let send_address_btn = Self::button(
            "Send",
            if enable {
                Some(Message::SendToAddress)
            } else {
                None
            },
        )
        .width(100);

        let amount_input = {
            let mut input = TextInput::new("amount", &self.send_amount).width(100);
            if enable {
                input = input.on_input(Message::AmountSend);
            }
            input
        };

        let address_input = {
            let mut input = TextInput::new("address", &self.send_address).width(Length::Fill);
            if enable {
                input = input.on_input(Message::AddressSend);
            }
            input
        };

        let enable = !self.send_wip && self.connected && !self.send_every_blocks_enabled;
        let send_descriptor_btn = Self::button(
            "Send",
            if enable {
                Some(Message::SendToDescriptor)
            } else {
                None
            },
        )
        .width(100);

        let count_input = {
            let mut input = TextInput::new("count", &self.send_count);
            if enable {
                input = input.on_input(Message::CountSend);
            }
            input
        };

        let min_input = {
            let mut input = TextInput::new("min", &self.send_min);
            if enable {
                input = input.on_input(Message::MinSend);
            }
            input
        };

        let max_input = {
            let mut input = TextInput::new("max", &self.send_max);
            if enable {
                input = input.on_input(Message::MaxSend);
            }
            input
        };

        let descriptor_input = {
            let mut input = TextInput::new("descriptor", &self.send_descriptor);
            if enable {
                input = input.on_input(Message::DescriptorSend);
            }
            input
        };

        let descriptor_index_input = {
            let mut input = TextInput::new("start index", &self.send_descriptor_index).width(100);
            if enable {
                input = input.on_input(Message::DescriptorIndexSend);
            }
            input
        };

        let every_block_checkbox = Checkbox::new("", self.send_every_blocks_enabled)
            .on_toggle_maybe(if self.connected {
                Some(Message::ToggleEveryBlock)
            } else {
                None
            });

        let every_block_input = {
            let mut input = TextInput::new("blocks", &self.send_every_blocks).width(120);
            let enable = match self.autoblock_wip {
                true => !self.send_every_blocks_enabled,
                false => self.send_every_blocks_enabled && !self.send_wip,
            };
            let enable = enable && self.connected;
            if enable {
                input = input.on_input(Message::BlockSend);
            }
            input
        };

        let col = Column::new()
            .push_maybe(balance)
            .push(Space::with_height(5))
            .push(Rule::horizontal(5))
            .push(Space::with_height(5))
            .push(
                Row::new()
                    .push(send_address_btn)
                    .push(Space::with_width(10))
                    .push(amount_input)
                    .push(Text::new(" BTC to "))
                    .push(address_input)
                    .align_items(iced::alignment::Alignment::Center),
            )
            .push(Space::with_height(5))
            .push(Rule::horizontal(5))
            .push(Space::with_height(5))
            .push(
                Row::new()
                    .push(send_descriptor_btn)
                    .push(Space::with_width(10))
                    .push(count_input)
                    .push(Text::new(" x "))
                    .push(min_input)
                    .push(Text::new(" - "))
                    .push(max_input)
                    .push(Text::new(" BTC "))
                    .align_items(iced::alignment::Alignment::Center),
            )
            .push(Space::with_height(5))
            .push(
                Row::new()
                    .push(Text::new(" to "))
                    .push(descriptor_input)
                    .push(Space::with_width(10))
                    .push(descriptor_index_input)
                    .align_items(iced::alignment::Alignment::Center),
            )
            .push(Space::with_height(5))
            .push(
                Row::new()
                    .push(every_block_checkbox)
                    .push(Text::new(" every "))
                    .push(every_block_input)
                    .push(Text::new(" blocks "))
                    .align_items(iced::alignment::Alignment::Center),
            )
            .push(Rule::horizontal(5))
            .push(Space::with_height(5));

        Container::new(col)
    }

    pub fn console_panel(&self) -> Container<Message> {
        let console = TextEditor::new(&self.console).on_action(Message::ConsoleEdit);

        Container::new(scrollable(console).height(300))
    }

    pub fn u32_checked(input: String, output: &mut String, max: u32) {
        if let Ok(blocks) = u32::from_str(&input) {
            if blocks <= max {
                *output = input;
            }
        } else if input.is_empty() {
            *output = input;
        }
    }
    pub fn amount_checked(input: String, output: &mut String) {
        if Amount::from_str_in(&input, miniscript::bitcoin::Denomination::Bitcoin).is_ok()
            || input.is_empty()
        {
            *output = input;
        }
    }
}

impl Application for Gui {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = Flags;

    fn new(flags: Self::Flags) -> (Self, Command<Message>) {
        let gui = Gui {
            receiver: flags.receiver,
            sender: flags.sender,
            config: Config::new(),
            block_height: Some(0),
            balance: Some(Amount::ZERO),
            generate_blocks: "".to_string(),
            generate_address: "".to_string(),
            generate_descriptor: "".to_string(),
            generate_descriptor_index: "".to_string(),
            send_amount: "".to_string(),
            send_count: "".to_string(),
            send_min: "".to_string(),
            send_max: "".to_string(),
            send_address: "".to_string(),
            send_descriptor: "".to_string(),
            send_descriptor_index: "".to_string(),
            send_every_blocks: "".to_string(),
            send_every_blocks_enabled: false,
            connected: false,
            autoblock_wip: false,
            autoblock_blocks: "1".to_string(),
            autoblocks_timeframe: TimeFrame::Second,
            generate_wip: false,
            send_wip: false,
            generate_target: GenerateTarget::Address,
            console: Content::new(),
        };

        (gui, Command::none())
    }

    fn title(&self) -> String {
        "Minta".to_string()
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Bitcoind(message) => match message {
                BitcoinMessage::UpdateBlockchainTip(block_height) => {
                    self.block_height = Some(block_height)
                }
                BitcoinMessage::UpdateBalance(amount) => self.balance = Some(amount),
                BitcoinMessage::GenerateResponse(success) => {
                    self.generate_wip = false;
                    if !success {
                        self.print("Fail to generate!")
                    }
                }
                BitcoinMessage::SendResponse(success) => {
                    self.send_wip = false;
                    if !success {
                        self.print("Fail to send!")
                    }
                }
                BitcoinMessage::SendMessage(msg) => {
                    self.print(&msg);
                }
                BitcoinMessage::Connected(connected) => {
                    self.connected = connected;
                    if connected {
                        if let Err(e) = self.config.save() {
                            self.print(&e);
                        }
                    }
                }
                BitcoinMessage::MinerStopped => self.autoblock_wip = false,
                BitcoinMessage::IncrementSendDescriptorIndex => {
                    if let Ok(index) = u32::from_str(&self.send_descriptor_index) {
                        let index = index.wrapping_add(1);
                        self.send_descriptor_index = index.to_string();
                    }
                }
                BitcoinMessage::IncrementGenerateDescriptorIndex => {
                    if let Ok(index) = u32::from_str(&self.generate_descriptor_index) {
                        let index = index.wrapping_add(1);
                        self.generate_descriptor_index = index.to_string();
                    }
                }
                _ => {}
            },

            // text Inputs
            Message::BitcoindAddress(address) => self.config.bitcoind.address = address,
            Message::User(user) => self.config.bitcoind.user = user,
            Message::Password(pass) => self.config.bitcoind.password = pass,
            Message::CookiePath(path) => self.config.bitcoind.cookie_path = path,
            Message::BlocksGenerate(blocks) => {
                Self::u32_checked(blocks, &mut self.generate_blocks, 10_000)
            }
            Message::AddressGenerate(address) => self.generate_address = address,
            Message::DescriptorGenerate(descriptor) => self.generate_descriptor = descriptor,
            Message::DescriptorIndexGenerate(index) => {
                Self::u32_checked(index, &mut self.generate_descriptor_index, MAX_DERIV)
            }
            Message::AmountSend(amount) => Self::amount_checked(amount, &mut self.send_amount),
            Message::CountSend(count) => Self::u32_checked(count, &mut self.send_count, u32::MAX),
            Message::AddressSend(address) => self.send_address = address,
            Message::DescriptorSend(descriptor) => self.send_descriptor = descriptor,
            Message::DescriptorIndexSend(index) => {
                Self::u32_checked(index, &mut self.send_descriptor_index, MAX_DERIV)
            }
            Message::MinSend(min) => Self::amount_checked(min, &mut self.send_min),
            Message::MaxSend(max) => Self::amount_checked(max, &mut self.send_max),
            Message::BlockSend(blocks) => {
                Self::u32_checked(blocks, &mut self.send_every_blocks, 10_000)
            }
            Message::AutoblockBlocks(blocks) => {
                Self::u32_checked(blocks, &mut self.autoblock_blocks, 1_000)
            }
            Message::AutoblockTimeframe(tf) => self.autoblocks_timeframe = tf,

            // Buttons
            Message::ConnectRpcAuth => self.connect_rpc_auth(),
            Message::ConnectCookie => self.connect_cookie(),
            Message::Disconnect => self.disconnect(),
            Message::StartAutoblock => {
                self.start_auto_block();
            }
            Message::StopAutoblock => self.stop_auto_block(),
            Message::GenerateToAddress => self.generate_to_address(),
            Message::GenerateToSelf => self.generate_to_self(),
            Message::GenerateToRandom => self.generate_to_random(),
            Message::GenerateToDescriptor => self.generate_to_descriptor(),
            Message::SendToAddress => self.send_to_address(),
            Message::SendToDescriptor => self.send_to_descriptor(),
            Message::SelectRpcAuth(selected) => {
                if selected {
                    self.config.bitcoind.auth_type = AuthMethod::RpcAuth;
                } else {
                    self.config.bitcoind.auth_type = AuthMethod::Cookie;
                }
            }
            Message::SelectCookie(selected) => {
                if !selected {
                    self.config.bitcoind.auth_type = AuthMethod::RpcAuth;
                } else {
                    self.config.bitcoind.auth_type = AuthMethod::Cookie;
                }
            }
            Message::ToggleEveryBlock(enable) => {
                self.toggle_every_blocks(enable);
            }
            Message::KeyPressed(Key::Tab(shift)) => {
                if shift {
                    return focus_previous();
                } else {
                    return focus_next();
                }
            }
            Message::ConsoleEdit(_) => {}
            Message::GenerateTarget(target) => self.generate_target = target,
        }

        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let main_frame = Column::new()
            .push(self.auth_panel())
            .push(Space::with_height(5))
            .push(Rule::horizontal(4))
            .push(Space::with_height(5))
            .push(self.auto_block_panel())
            .push(Space::with_height(5))
            .push(Rule::horizontal(4))
            .push(Space::with_height(5))
            .push(self.generate_panel())
            .push(Space::with_height(5))
            .push(Rule::horizontal(4))
            .push(Space::with_height(5))
            .push(self.send_panel())
            .push(Space::with_height(5))
            .push(self.console_panel())
            .push(Space::with_height(5))
            .padding(5);

        main_frame.into()
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        let bitcoind = iced::Subscription::from_recipe(BitcoindListener {
            receiver: self.receiver.clone(),
        });
        let keys = iced::event::listen_with(|event, status| match (&event, status) {
            (
                iced::event::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
                    modifiers,
                    ..
                }),
                iced::event::Status::Ignored,
            ) => Some(Message::KeyPressed(Key::Tab(modifiers.shift()))),
            _ => None,
        });
        Subscription::batch(vec![bitcoind, keys])
    }
}
