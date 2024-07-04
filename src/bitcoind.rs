use std::{fmt::format, path::PathBuf, str::FromStr, time::Duration};

use bitcoincore_rpc::{
    jsonrpc::{
        error::RpcError,
        serde_json::{from_str, Value},
    },
    Auth, Client, RpcApi,
};
use miniscript::{
    bitcoin::{
        address::NetworkUnchecked, key::Secp256k1, secp256k1::All, Address, Amount, Network,
        PrivateKey,
    },
    Descriptor, DescriptorPublicKey,
};
use rand::Rng;
use regex::Regex;

use crate::{gui::Message, gui::Message::Bitcoind, listener, service::ServiceFn};

const WALLET_NAME: &str = "regtest";

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
    pub blocks: u32,
    pub address: Address,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct GenerateToDescriptor {
    pub blocks: u32,
    pub descriptor: String,
    pub start_index: u32,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct SendToAddress {
    pub amount: Amount,
    pub address: Address,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct SendToDescriptor {
    pub count: u32,
    pub amount_min: Amount,
    pub amount_max: Amount,
    pub descriptor: String,
    pub start_index: u32,
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
    /// Connect to bitcoind
    Connect,
    /// Disconnect from bitcoind
    Disconnect,

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
    /// Send to bitcoin descriptor
    SendToDescriptor(SendToDescriptor),

    // Service -> GUI
    UpdateBlockchainTip(u64),
    UpdateBalance(Amount),
    GenerateResponse(bool),
    SendResponse(bool),
    SendMessage(String),
    Connected(bool),
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
    GetNewAddress,
    Rpc(bitcoincore_rpc::Error),
}

#[allow(unused)]
pub struct BitcoinD {
    sender: async_channel::Sender<BitcoinMessage>,
    receiver: std::sync::mpsc::Receiver<BitcoinMessage>,
    client: Option<Client>,
    wallet_client: Option<Client>,
    address: Option<String>,
    auth: Option<AuthMethod>,
    mining_busy: bool,
    secp: miniscript::bitcoin::secp256k1::Secp256k1<All>,
}

#[allow(unused)]
impl BitcoinD {
    pub fn set_credentials(&mut self, address: String, auth: AuthMethod) {
        self.address = Some(address);
        self.auth = Some(auth);
    }

    pub fn connect(&self) -> Result<(Client, Client), Error> {
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

            let wallet_address = format!("{}/wallet/{}", address, WALLET_NAME);
            let wallet_client = match auth {
                AuthMethod::Cookie { cookie_path } => {
                    let cookie_path = PathBuf::from(cookie_path);
                    Client::new(&wallet_address, Auth::CookieFile(cookie_path))
                }
                AuthMethod::RpcAuth { user, password } => Client::new(
                    address,
                    Auth::UserPass(user.to_string(), password.to_string()),
                ),
            }
            .map_err(Error::Rpc)?;

            wallet_client
                .call::<bool>("settxfee", &[0.0001.into()])
                .map_err(Error::Rpc)?;

            match client {
                Ok(client) => match client.load_wallet(WALLET_NAME) {
                    Ok(w) => Ok((client, wallet_client)),
                    Err(e) => {
                        if let bitcoincore_rpc::Error::JsonRpc(
                            bitcoincore_rpc::jsonrpc::Error::Rpc(RpcError { code, .. }),
                        ) = e
                        {
                            // -18 => wallet does not exist
                            if code == -18 {
                                client
                                    .create_wallet(WALLET_NAME, None, None, None, None)
                                    .map_err(Error::Rpc)?;
                            } else if code != -35 {
                                // -35 => wallet already loaded
                                return Err(Error::Rpc(e));
                            }
                            Ok((client, wallet_client))
                        } else {
                            Err(Error::Rpc(e))
                        }
                    }
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

    pub fn disconnect(&mut self) {
        self.client = None;
        self.wallet_client = None;
        self.auth = None;
        self.send_to_gui(BitcoinMessage::Connected(false));
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
        if let Some(client) = self.wallet_client.as_ref() {
            client.get_balance(None, None).map_err(Error::Rpc)
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn get_random_address(&self) -> Address {
        let prv = PrivateKey::generate(Network::Regtest);
        let pb = prv.public_key(&self.secp);
        Address::p2pkh(pb, Network::Regtest)
    }

    pub fn generate(&self, blocks: u32) -> Result<(), Error> {
        let address = self.get_random_address();
        self.generate_to_address(GenerateToAddress { blocks, address })?;
        Ok(())
    }

    pub fn generate_to_address(&self, params: GenerateToAddress) -> Result<(), Error> {
        if let Some(client) = self.client.as_ref() {
            client
                .generate_to_address(params.blocks as u64, &params.address)
                .map_err(Error::Rpc)?;
            Ok(())
        } else {
            Err(Error::NotConnected)
        }
    }

    // pub fn get_new_address(&self) -> Result<Address<NetworkUnchecked>, Error> {
    //     if let Some(client) = self.client.as_ref() {
    //         let info = client.
    //             .map_err(|e| Error::Rpc(e))?
    //             .
    //     } else {
    //         Err(Error::GetNewAddress)
    //     }
    // }

    pub fn generate_to_self(&self, blocks: u32) -> Result<(), Error> {
        if let Some(client) = self.wallet_client.as_ref() {
            let address = client
                .get_new_address(None, None)
                .map_err(Error::Rpc)?
                .assume_checked();
            self.generate_to_address(GenerateToAddress { blocks, address })
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn generate_to_descriptor(&self, params: GenerateToDescriptor) -> Result<(), Error> {
        let (start, end) = (params.start_index, params.start_index + params.blocks);
        let descriptor =
            Descriptor::from_str(&params.descriptor).map_err(|_| Error::ParseDescriptor)?;

        for index in start..end {
            let address = self.address_from_descriptor(descriptor.clone(), index)?;
            self.generate_to_address(GenerateToAddress { blocks: 1, address })?;
        }
        Ok(())
    }

    pub fn address_from_descriptor(
        &self,
        descriptor: Descriptor<DescriptorPublicKey>,
        index: u32,
    ) -> Result<Address, Error> {
        descriptor
            .into_single_descriptors()
            .map_err(|_| Error::ParseDescriptor)?
            .into_iter()
            // we take the first multipath as receive path
            .next()
            .ok_or(Error::ParseDescriptor)?
            .derived_descriptor(&self.secp, index)
            .map_err(|_| Error::DeriveDescriptor)?
            .address(Network::Regtest)
            .map_err(|_| Error::DeriveDescriptor)
    }

    pub fn send_to_address(&self, params: SendToAddress) -> Result<(), Error> {
        if let Some(client) = self.wallet_client.as_ref() {
            client
                .send_to_address(
                    &params.address,
                    params.amount,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .map_err(Error::Rpc)?;
            Ok(())
        } else {
            Err(Error::NotConnected)
        }
    }

    pub fn send_to_descriptor(&self, params: SendToDescriptor) -> Result<(), Error> {
        let (start, end) = (params.start_index, params.start_index + params.count);
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(&params.descriptor)
            .map_err(|_| Error::DeriveDescriptor)?;
        for index in start..end {
            let amount = Self::random_amount(params.amount_min, params.amount_max);
            let address = self.address_from_descriptor(descriptor.clone(), index)?;
            self.send_to_address(SendToAddress { amount, address })?;
        }
        Ok(())
    }

    pub fn random_amount(min: Amount, max: Amount) -> Amount {
        let mut rng = rand::thread_rng();
        let (min, max) = (min.to_sat(), max.to_sat());
        let random = rng.gen_range(min..max);
        Amount::from_sat(random)
    }

    pub fn handle_connect(&mut self) {
        if !self.is_connected() {
            match self.connect() {
                Ok(client) => {
                    self.client = Some(client.0);
                    self.wallet_client = Some(client.1);
                    self.send_to_gui(BitcoinMessage::Connected(true));
                    log::info!("Connected!");
                }
                Err(e) => {
                    log::error!("Fail to connect: {:?}", e);
                    self.send_to_gui(BitcoinMessage::SendMessage(format!(
                        "Fail to connect: {:?}",
                        e
                    )));
                    self.send_to_gui(BitcoinMessage::Connected(false));
                }
            }
        } else {
            self.send_to_gui(BitcoinMessage::SendMessage(
                "Already connected!".to_string(),
            ));
            self.send_to_gui(BitcoinMessage::Connected(true));

            log::error!("Already connected!");
        }
    }

    pub fn handle_message(&mut self, msg: BitcoinMessage) {
        log::info!("BitcoinD.handle_message({:?})", msg);
        match (msg, &self.mining_busy) {
            (BitcoinMessage::Ping { address, auth }, _) => todo!(),
            (BitcoinMessage::SetCredentials { address, auth }, _) => {
                self.address = Some(address);
                self.auth = Some(auth);
            }
            (BitcoinMessage::Connect, _) => {
                self.handle_connect();
                self.update_data();
            }
            (BitcoinMessage::Disconnect, _) => {
                if self.is_connected() {
                    self.disconnect()
                }
            }
            (BitcoinMessage::Generate(blocks), false) => {
                self.mining_busy = true;
                if let Err(e) = self.generate(blocks) {
                    self.send_to_gui(BitcoinMessage::SendMessage(format!("{:?}", e)));
                    self.send_to_gui(BitcoinMessage::GenerateResponse(false));
                } else {
                    self.send_to_gui(BitcoinMessage::GenerateResponse(true));
                }
                self.update_data();
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
                self.update_data();
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
                self.update_data();
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
                self.update_data();
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
            (BitcoinMessage::SendToDescriptor(params), _) => {
                if let Err(e) = self.send_to_descriptor(params) {
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
        let sender = self.sender.clone();
        tokio::spawn(async move {
            if sender.send(message).await.is_err() {
                log::debug!("send_to_gui() -> Fail to send Message")
            };
        });
    }

    pub fn update_data(&self) {
        if let Ok(blocks) = self.get_block_height() {
            self.send_to_gui(BitcoinMessage::UpdateBlockchainTip(blocks))
        }
        if let Ok(balance) = self.get_balance() {
            self.send_to_gui(BitcoinMessage::UpdateBalance(balance))
        }
    }

    pub async fn start(mut self) {
        self.run().await;
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
            wallet_client: None,
            address: None,
            auth: None,
            mining_busy: false,
            secp: miniscript::bitcoin::secp256k1::Secp256k1::new(),
        }
    }

    async fn run(&mut self) {
        loop {
            if let Ok(msg) = self.receiver.try_recv() {
                self.handle_message(msg);
            }
            tokio::time::sleep(Duration::from_nanos(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use miniscript::Descriptor;

    use super::*;

    #[test]
    fn parse_descriptor() {
        const DESCRIPTOR: &str = "wsh(or_d(pk([9c32dc88/48'/1'/0'/2']tpubDEUUVSJyh6t12FbNhmmYa1M39AiD2VKGBaGT54aPz2xVF5Kg1dx3XSb5T4nKBakEz8ypy35fYVAZgBc7MVwQ2qEZEZRqDbvDu8w5AZVu4q2/<0;1>/*),and_v(v:pkh([9c32dc88/48'/1'/0'/2']tpubDEUUVSJyh6t12FbNhmmYa1M39AiD2VKGBaGT54aPz2xVF5Kg1dx3XSb5T4nKBakEz8ypy35fYVAZgBc7MVwQ2qEZEZRqDbvDu8w5AZVu4q2/<2;3>/*),older(65535))))#686a8fmh";

        let secp = miniscript::bitcoin::secp256k1::Secp256k1::new();

        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(DESCRIPTOR).unwrap();

        let addr = descriptor
            .into_single_descriptors()
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .derived_descriptor(&secp, 1)
            .unwrap()
            .address(Network::Regtest)
            .unwrap();

        println!("address: {}", addr);

        // let addr = BitcoinD::address_from_descriptor(DESCRIPTOR, 1, &secp);
    }
}
