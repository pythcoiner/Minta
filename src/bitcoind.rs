use std::{path::PathBuf, str::FromStr, time::Duration};

use bitcoincore_rpc::{Auth, Client, RpcApi};
use miniscript::{
    bitcoin::{Address, Amount},
    DescriptorPublicKey,
};

use crate::{gui::Message, gui::Message::Bitcoind, listener, service::ServiceFn};

listener!(BitcoindListener, BitcoinMessage, Message, Bitcoind);

#[derive(Debug, Clone)]
#[allow(unused)]
pub enum AuthMethod {
    Cookie { cookie_path: String },
    RpcAuth { user: String, password: String },
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct GenerateToAddress {
    blocks: u32,
    address: String,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct GenerateToDescriptor {
    blocks: u32,
    descriptor: String,
    start_index: u32,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct SendToAddress {
    amount: Amount,
    address: String,
    repeat: u32,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct SendToDescriptor {
    count: u32,
    amount_min: Amount,
    amount_max: Amount,
    descriptor: String,
    start_index: u32,
    repeat: u32,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub enum BitcoinMessage {
    // GUI -> Service
    /// Check connection to bitcoind
    Ping {
        address: String,
        auth: AuthMethod,
    },
    /// Set credentials
    SetCredentials {
        address: String,
        auth: AuthMethod,
    },

    /// Generate blocks to unknown address
    Generate(u32),
    /// Generate blocks to rpcwallet 'regtest'
    GenerateToSelf(u32),
    /// Generate blocks to a specified address
    GenerateToAddress(GenerateToAddress),
    /// Generate to a descriptor
    GenerateToDescriptor(GenerateToDescriptor),

    /// Send bitcoins to an address
    SendToAddress(SendToAddress),
    // Service -> GUI
    UpdateBlockchainTip(u64),
    UpdateBalance(Amount),
    GenerateResponse(bool),
    SendResponse(bool),
    SendMessage(String),
}

#[derive(Debug)]
#[allow(unused)]
pub enum Error {
    WrongAddress,
    WrongAuth,
    CredentialMissing,
    NotConnected,
    ParseAddressFail,
    ParseDescriptor,
    DeriveDescriptor,
    Network,
    SetTxFee,
    Rpc(bitcoincore_rpc::Error),
}

#[derive(Debug)]
#[allow(unused)]
pub struct BitcoinD {
    sender: async_channel::Sender<BitcoinMessage>,
    receiver: std::sync::mpsc::Receiver<BitcoinMessage>,
    client: Option<Client>,
    address: Option<String>,
    auth: Option<AuthMethod>,
    mining_busy: bool,
}

#[allow(unused)]
impl BitcoinD {
    pub fn set_credentials(&mut self, address: String, auth: AuthMethod) {
        self.address = Some(address);
        self.auth = Some(auth);
    }

    pub fn connect(&self) -> Result<Client, Error> {
        if let (Some(address), Some(auth)) = (&self.address, &self.auth) {
            let client = match auth {
                AuthMethod::Cookie { cookie_path } => {
                    let cookie_path = PathBuf::from(cookie_path);
                    Client::new(address, Auth::CookieFile(cookie_path))
                }
                AuthMethod::RpcAuth { user, password } => Client::new(
                    address,
                    Auth::UserPass(user.to_string(), password.to_string()),
                ),
            };

            match client {
                Ok(client) => match client.list_wallets() {
                    Ok(w) => {
                        if !w.contains(&"regtest".to_string()) {
                            client
                                .create_wallet("regtest", None, None, None, None)
                                .map_err(Error::Rpc)?;
                        }
                        client.load_wallet("regtest").map_err(Error::Rpc)?;
                        client
                            .call::<bool>("settxfee", &[0.0001.into()])
                            .map_err(|_| Error::SetTxFee)?;
                        Ok(client)
                    }
                    Err(e) => Err(Error::Rpc(e)),
                },
                Err(e) => Err(Error::Rpc(e)),
            }
        } else {
            Err(Error::CredentialMissing)
        }
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    pub fn get_block_height(&self) -> Result<u64, Error> {
        if let Some(client) = self.client.as_ref() {
            match client.get_blockchain_info() {
                Ok(info) => Ok(info.blocks),
                Err(e) => Err(Error::Rpc(e)),
            }
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn get_balance(&self) -> Result<Amount, Error> {
        if let Some(client) = self.client.as_ref() {
            client.get_balance(None, None).map_err(Error::Rpc)
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn get_random_address() -> Address {
        todo!()
    }

    pub fn generate(&self, blocks: u32) -> Result<(), Error> {
        let address = Self::get_random_address();
        self.generate_to_address(GenerateToAddress {
            blocks,
            address: address.to_string(),
        })?;
        Ok(())
    }

    pub fn generate_to_address(&self, params: GenerateToAddress) -> Result<(), Error> {
        let address = Address::from_str(&params.address)
            .map_err(|_| Error::ParseAddressFail)?
            .assume_checked();
        if let Some(client) = self.client.as_ref() {
            client
                .generate_to_address(params.blocks as u64, &address)
                .map_err(Error::Rpc)?;
            Ok(())
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn generate_to_self(&self, blocks: u32) -> Result<(), Error> {
        if let Some(client) = self.client.as_ref() {
            client.generate(blocks as u64, None).map_err(Error::Rpc)?;
            Ok(())
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn generate_to_descriptor(&self, params: GenerateToDescriptor) -> Result<(), Error> {
        let descriptor =
            miniscript::Descriptor::<DescriptorPublicKey>::from_str(&params.descriptor)
                .map_err(|_| Error::ParseDescriptor)?;
        let (start, end) = (params.start_index, params.start_index + params.blocks);
        for index in start..end {
            let address = descriptor
                .at_derivation_index(index)
                .map_err(|_| Error::DeriveDescriptor)?
                .address(miniscript::bitcoin::Network::Regtest)
                .map_err(|_| Error::Network)?;

            self.generate_to_address(GenerateToAddress {
                blocks: 1,
                address: address.to_string(),
            })?;
        }
        Ok(())
    }

    pub fn send_to_address(&self, params: SendToAddress) -> Result<(), Error> {
        let address = Address::from_str(&params.address)
            .map_err(|_| Error::ParseAddressFail)?
            .assume_checked();
        if let Some(client) = self.client.as_ref() {
            client
                .send_to_address(&address, params.amount, None, None, None, None, None, None)
                .map_err(Error::Rpc)?;
            Ok(())
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn handle_message(&mut self, msg: BitcoinMessage) {
        match (msg, &self.mining_busy) {
            (BitcoinMessage::Ping { address, auth }, _) => todo!(),
            (BitcoinMessage::SetCredentials { address, auth }, _) => {
                self.address = Some(address);
                self.auth = Some(auth);
            }
            (BitcoinMessage::Generate(blocks), false) => {
                self.mining_busy = true;
                if let Err(e) = self.generate(blocks) {
                    self.send_to_gui(BitcoinMessage::SendMessage(format!("{:?}", e)));
                    self.send_to_gui(BitcoinMessage::GenerateResponse(false));
                } else {
                    self.send_to_gui(BitcoinMessage::GenerateResponse(true));
                }
                self.update();
                self.mining_busy = false;
            }
            (BitcoinMessage::GenerateToSelf(blocks), false) => {
                self.mining_busy = true;
                if let Err(e) = self.generate_to_self(blocks) {
                    self.send_to_gui(BitcoinMessage::SendMessage(format!("{:?}", e)));
                    self.send_to_gui(BitcoinMessage::GenerateResponse(false));
                } else {
                    self.send_to_gui(BitcoinMessage::GenerateResponse(true));
                }
                self.update();
                self.mining_busy = false;
            }
            (BitcoinMessage::GenerateToAddress(params), false) => {
                self.mining_busy = true;
                if let Err(e) = self.generate_to_address(params) {
                    self.send_to_gui(BitcoinMessage::SendMessage(format!("{:?}", e)));
                    self.send_to_gui(BitcoinMessage::GenerateResponse(false));
                } else {
                    self.send_to_gui(BitcoinMessage::GenerateResponse(true));
                }
                self.update();
                self.mining_busy = false;
            }
            (BitcoinMessage::GenerateToDescriptor(params), false) => {
                self.mining_busy = true;
                if let Err(e) = self.generate_to_descriptor(params) {
                    self.send_to_gui(BitcoinMessage::SendMessage(format!("{:?}", e)));
                    self.send_to_gui(BitcoinMessage::GenerateResponse(false));
                } else {
                    self.send_to_gui(BitcoinMessage::GenerateResponse(true));
                }
                self.update();
                self.mining_busy = false;
            }
            (BitcoinMessage::SendToAddress(params), _) => {
                if let Err(e) = self.send_to_address(params) {
                    self.send_to_gui(BitcoinMessage::SendMessage(format!("{:?}", e)));
                    self.send_to_gui(BitcoinMessage::SendResponse(false));
                } else {
                    self.send_to_gui(BitcoinMessage::SendResponse(true));
                }
            }

            _ => {} // BitcoinMessage::UpdateBlockchainTip(_) => todo!(),
                    // BitcoinMessage::UpdateBalance(_) => todo!(),
                    // BitcoinMessage::GenerateResponse(_) => todo!(),
                    // BitcoinMessage::SendResponse() => todo
        }
    }

    pub fn send_to_gui(&self, message: BitcoinMessage) {
        self.sender.send(message);
    }

    pub fn update(&self) {
        if let Ok(blocks) = self.get_block_height() {
            self.send_to_gui(BitcoinMessage::UpdateBlockchainTip(blocks))
        }
        if let Ok(balance) = self.get_balance() {
            self.send_to_gui(BitcoinMessage::UpdateBalance(balance))
        }
    }

    pub fn start(mut self) {
        self.run();
    }
}

impl ServiceFn<BitcoinMessage> for BitcoinD {
    fn new(
        sender: async_channel::Sender<BitcoinMessage>,
        receiver: std::sync::mpsc::Receiver<BitcoinMessage>,
    ) -> Self {
        BitcoinD {
            sender,
            receiver,
            client: None,
            address: None,
            auth: None,
            mining_busy: false,
        }
    }

    async fn run(&mut self) {
        loop {
            if let Ok(msg) = self.receiver.try_recv() {
                self.handle_message(msg);
            }
            tokio::time::sleep(Duration::from_nanos(10)).await;
        }
    }
}
