// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hash_types::Txid;
use bitcoin::{OutPoint, Script, Transaction, TxOut};

use crate::database::{BatchDatabase, BatchOperations, Database, SyncTime};
use crate::error::Error;
use crate::types::*;

use rusqlite::{named_params, Connection};

static MIGRATIONS: &[&str] = &[
    "CREATE TABLE version (version INTEGER)",
    "INSERT INTO version VALUES (1)",
    "CREATE TABLE script_pubkeys (keychain TEXT, child INTEGER, script BLOB);",
    "CREATE INDEX idx_keychain_child ON script_pubkeys(keychain, child);",
    "CREATE INDEX idx_script ON script_pubkeys(script);",
    "CREATE TABLE utxos (value INTEGER, keychain TEXT, vout INTEGER, txid BLOB, script BLOB);",
    "CREATE INDEX idx_txid_vout ON utxos(txid, vout);",
    "CREATE TABLE transactions (txid BLOB, raw_tx BLOB);",
    "CREATE INDEX idx_txid ON transactions(txid);",
    "CREATE TABLE transaction_details (txid BLOB, timestamp INTEGER, received INTEGER, sent INTEGER, fee INTEGER, height INTEGER, verified INTEGER DEFAULT 0);",
    "CREATE INDEX idx_txdetails_txid ON transaction_details(txid);",
    "CREATE TABLE last_derivation_indices (keychain TEXT, value INTEGER);",
    "CREATE UNIQUE INDEX idx_indices_keychain ON last_derivation_indices(keychain);",
    "CREATE TABLE checksums (keychain TEXT, checksum BLOB);",
    "CREATE INDEX idx_checksums_keychain ON checksums(keychain);",
    "CREATE TABLE sync_time (id INTEGER PRIMARY KEY, height INTEGER, timestamp INTEGER);",
    "ALTER TABLE transaction_details RENAME TO transaction_details_old;",
    "CREATE TABLE transaction_details (txid BLOB, timestamp INTEGER, received INTEGER, sent INTEGER, fee INTEGER, height INTEGER);",
    "INSERT INTO transaction_details SELECT txid, timestamp, received, sent, fee, height FROM transaction_details_old;",
    "DROP TABLE transaction_details_old;",
    "ALTER TABLE utxos ADD COLUMN is_spent;",
];

/// Sqlite database stored on filesystem
///
/// This is a permanent storage solution for devices and platforms that provide a filesystem.
/// [`crate::database`]
#[derive(Debug)]
pub struct SqliteDatabase {
    /// Path on the local filesystem to store the sqlite file
    pub path: String,
    /// A rusqlite connection object to the sqlite database
    pub connection: Connection,
}

impl SqliteDatabase {
    /// Instantiate a new SqliteDatabase instance by creating a connection
    /// to the database stored at path
    pub fn new(path: String) -> Self {
        let connection = get_connection(&path).unwrap();
        SqliteDatabase { path, connection }
    }
    fn insert_script_pubkey(
        &self,
        keychain: String,
        child: u32,
        script: &[u8],
    ) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached("INSERT INTO script_pubkeys (keychain, child, script) VALUES (:keychain, :child, :script)")?;
        statement.execute(named_params! {
            ":keychain": keychain,
            ":child": child,
            ":script": script
        })?;

        Ok(self.connection.last_insert_rowid())
    }
    fn insert_utxo(
        &self,
        value: u64,
        keychain: String,
        vout: u32,
        txid: &[u8],
        script: &[u8],
        is_spent: bool,
    ) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached("INSERT INTO utxos (value, keychain, vout, txid, script, is_spent) VALUES (:value, :keychain, :vout, :txid, :script, :is_spent)")?;
        statement.execute(named_params! {
            ":value": value,
            ":keychain": keychain,
            ":vout": vout,
            ":txid": txid,
            ":script": script,
            ":is_spent": is_spent,
        })?;

        Ok(self.connection.last_insert_rowid())
    }
    fn insert_transaction(&self, txid: &[u8], raw_tx: &[u8]) -> Result<i64, Error> {
        let mut statement = self
            .connection
            .prepare_cached("INSERT INTO transactions (txid, raw_tx) VALUES (:txid, :raw_tx)")?;
        statement.execute(named_params! {
            ":txid": txid,
            ":raw_tx": raw_tx,
        })?;

        Ok(self.connection.last_insert_rowid())
    }

    fn update_transaction(&self, txid: &[u8], raw_tx: &[u8]) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("UPDATE transactions SET raw_tx=:raw_tx WHERE txid=:txid")?;

        statement.execute(named_params! {
            ":txid": txid,
            ":raw_tx": raw_tx,
        })?;

        Ok(())
    }

    fn insert_transaction_details(&self, transaction: &TransactionDetails) -> Result<i64, Error> {
        let (timestamp, height) = match &transaction.confirmation_time {
            Some(confirmation_time) => (
                Some(confirmation_time.timestamp),
                Some(confirmation_time.height),
            ),
            None => (None, None),
        };

        let txid: &[u8] = &transaction.txid;

        let mut statement = self.connection.prepare_cached("INSERT INTO transaction_details (txid, timestamp, received, sent, fee, height) VALUES (:txid, :timestamp, :received, :sent, :fee, :height)")?;

        statement.execute(named_params! {
            ":txid": txid,
            ":timestamp": timestamp,
            ":received": transaction.received,
            ":sent": transaction.sent,
            ":fee": transaction.fee,
            ":height": height,
        })?;

        Ok(self.connection.last_insert_rowid())
    }

    fn update_transaction_details(&self, transaction: &TransactionDetails) -> Result<(), Error> {
        let (timestamp, height) = match &transaction.confirmation_time {
            Some(confirmation_time) => (
                Some(confirmation_time.timestamp),
                Some(confirmation_time.height),
            ),
            None => (None, None),
        };

        let txid: &[u8] = &transaction.txid;

        let mut statement = self.connection.prepare_cached("UPDATE transaction_details SET timestamp=:timestamp, received=:received, sent=:sent, fee=:fee, height=:height WHERE txid=:txid")?;

        statement.execute(named_params! {
            ":txid": txid,
            ":timestamp": timestamp,
            ":received": transaction.received,
            ":sent": transaction.sent,
            ":fee": transaction.fee,
            ":height": height,
        })?;

        Ok(())
    }

    fn insert_last_derivation_index(&self, keychain: String, value: u32) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached(
            "INSERT INTO last_derivation_indices (keychain, value) VALUES (:keychain, :value)",
        )?;

        statement.execute(named_params! {
            ":keychain": keychain,
            ":value": value,
        })?;

        Ok(self.connection.last_insert_rowid())
    }

    fn insert_checksum(&self, keychain: String, checksum: &[u8]) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached(
            "INSERT INTO checksums (keychain, checksum) VALUES (:keychain, :checksum)",
        )?;
        statement.execute(named_params! {
            ":keychain": keychain,
            ":checksum": checksum,
        })?;

        Ok(self.connection.last_insert_rowid())
    }

    fn update_last_derivation_index(&self, keychain: String, value: u32) -> Result<(), Error> {
        let mut statement = self.connection.prepare_cached(
            "INSERT INTO last_derivation_indices (keychain, value) VALUES (:keychain, :value) ON CONFLICT(keychain) DO UPDATE SET value=:value WHERE keychain=:keychain",
        )?;

        statement.execute(named_params! {
            ":keychain": keychain,
            ":value": value,
        })?;

        Ok(())
    }

    fn update_sync_time(&self, data: SyncTime) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached(
            "INSERT INTO sync_time (id, height, timestamp) VALUES (0, :height, :timestamp) ON CONFLICT(id) DO UPDATE SET height=:height, timestamp=:timestamp WHERE id = 0",
        )?;

        statement.execute(named_params! {
            ":height": data.block_time.height,
            ":timestamp": data.block_time.timestamp,
        })?;

        Ok(self.connection.last_insert_rowid())
    }

    fn select_script_pubkeys(&self) -> Result<Vec<Script>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT script FROM script_pubkeys")?;
        let mut scripts: Vec<Script> = vec![];
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let raw_script: Vec<u8> = row.get(0)?;
            scripts.push(raw_script.into());
        }

        Ok(scripts)
    }

    fn select_script_pubkeys_by_keychain(&self, keychain: String) -> Result<Vec<Script>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT script FROM script_pubkeys WHERE keychain=:keychain")?;
        let mut scripts: Vec<Script> = vec![];
        let mut rows = statement.query(named_params! {":keychain": keychain})?;
        while let Some(row) = rows.next()? {
            let raw_script: Vec<u8> = row.get(0)?;
            scripts.push(raw_script.into());
        }

        Ok(scripts)
    }

    fn select_script_pubkey_by_path(
        &self,
        keychain: String,
        child: u32,
    ) -> Result<Option<Script>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT script FROM script_pubkeys WHERE keychain=:keychain AND child=:child",
        )?;
        let mut rows = statement.query(named_params! {":keychain": keychain,":child": child})?;

        match rows.next()? {
            Some(row) => {
                let script: Vec<u8> = row.get(0)?;
                let script: Script = script.into();
                Ok(Some(script))
            }
            None => Ok(None),
        }
    }

    fn select_script_pubkey_by_script(
        &self,
        script: &[u8],
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT keychain, child FROM script_pubkeys WHERE script=:script")?;
        let mut rows = statement.query(named_params! {":script": script})?;
        match rows.next()? {
            Some(row) => {
                let keychain: String = row.get(0)?;
                let keychain: KeychainKind = serde_json::from_str(&keychain)?;
                let child: u32 = row.get(1)?;
                Ok(Some((keychain, child)))
            }
            None => Ok(None),
        }
    }

    fn select_utxos(&self) -> Result<Vec<LocalUtxo>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT value, keychain, vout, txid, script, is_spent FROM utxos")?;
        let mut utxos: Vec<LocalUtxo> = vec![];
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let value = row.get(0)?;
            let keychain: String = row.get(1)?;
            let vout = row.get(2)?;
            let txid: Vec<u8> = row.get(3)?;
            let script: Vec<u8> = row.get(4)?;
            let is_spent: bool = row.get(5)?;

            let keychain: KeychainKind = serde_json::from_str(&keychain)?;

            utxos.push(LocalUtxo {
                outpoint: OutPoint::new(deserialize(&txid)?, vout),
                txout: TxOut {
                    value,
                    script_pubkey: script.into(),
                },
                keychain,
                is_spent,
            })
        }

        Ok(utxos)
    }

    fn select_utxo_by_outpoint(&self, txid: &[u8], vout: u32) -> Result<Option<LocalUtxo>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT value, keychain, script, is_spent FROM utxos WHERE txid=:txid AND vout=:vout",
        )?;
        let mut rows = statement.query(named_params! {":txid": txid,":vout": vout})?;
        match rows.next()? {
            Some(row) => {
                let value: u64 = row.get(0)?;
                let keychain: String = row.get(1)?;
                let keychain: KeychainKind = serde_json::from_str(&keychain)?;
                let script: Vec<u8> = row.get(2)?;
                let script_pubkey: Script = script.into();
                let is_spent: bool = row.get(3)?;

                Ok(Some(LocalUtxo {
                    outpoint: OutPoint::new(deserialize(txid)?, vout),
                    txout: TxOut {
                        value,
                        script_pubkey,
                    },
                    keychain,
                    is_spent,
                }))
            }
            None => Ok(None),
        }
    }

    fn select_transactions(&self) -> Result<Vec<Transaction>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT raw_tx FROM transactions")?;
        let mut txs: Vec<Transaction> = vec![];
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let raw_tx: Vec<u8> = row.get(0)?;
            let tx: Transaction = deserialize(&raw_tx)?;
            txs.push(tx);
        }
        Ok(txs)
    }

    fn select_transaction_by_txid(&self, txid: &[u8]) -> Result<Option<Transaction>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT raw_tx FROM transactions WHERE txid=:txid")?;
        let mut rows = statement.query(named_params! {":txid": txid})?;
        match rows.next()? {
            Some(row) => {
                let raw_tx: Vec<u8> = row.get(0)?;
                let tx: Transaction = deserialize(&raw_tx)?;
                Ok(Some(tx))
            }
            None => Ok(None),
        }
    }

    fn select_transaction_details_with_raw(&self) -> Result<Vec<TransactionDetails>, Error> {
        let mut statement = self.connection.prepare_cached("SELECT transaction_details.txid, transaction_details.timestamp, transaction_details.received, transaction_details.sent, transaction_details.fee, transaction_details.height, transactions.raw_tx FROM transaction_details, transactions WHERE transaction_details.txid = transactions.txid")?;
        let mut transaction_details: Vec<TransactionDetails> = vec![];
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let txid: Vec<u8> = row.get(0)?;
            let txid: Txid = deserialize(&txid)?;
            let timestamp: Option<u64> = row.get(1)?;
            let received: u64 = row.get(2)?;
            let sent: u64 = row.get(3)?;
            let fee: Option<u64> = row.get(4)?;
            let height: Option<u32> = row.get(5)?;
            let raw_tx: Option<Vec<u8>> = row.get(7)?;
            let tx: Option<Transaction> = match raw_tx {
                Some(raw_tx) => {
                    let tx: Transaction = deserialize(&raw_tx)?;
                    Some(tx)
                }
                None => None,
            };

            let confirmation_time = match (height, timestamp) {
                (Some(height), Some(timestamp)) => Some(BlockTime { height, timestamp }),
                _ => None,
            };

            transaction_details.push(TransactionDetails {
                transaction: tx,
                txid,
                received,
                sent,
                fee,
                confirmation_time,
            });
        }
        Ok(transaction_details)
    }

    fn select_transaction_details(&self) -> Result<Vec<TransactionDetails>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT txid, timestamp, received, sent, fee, height FROM transaction_details",
        )?;
        let mut transaction_details: Vec<TransactionDetails> = vec![];
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let txid: Vec<u8> = row.get(0)?;
            let txid: Txid = deserialize(&txid)?;
            let timestamp: Option<u64> = row.get(1)?;
            let received: u64 = row.get(2)?;
            let sent: u64 = row.get(3)?;
            let fee: Option<u64> = row.get(4)?;
            let height: Option<u32> = row.get(5)?;

            let confirmation_time = match (height, timestamp) {
                (Some(height), Some(timestamp)) => Some(BlockTime { height, timestamp }),
                _ => None,
            };

            transaction_details.push(TransactionDetails {
                transaction: None,
                txid,
                received,
                sent,
                fee,
                confirmation_time,
            });
        }
        Ok(transaction_details)
    }

    fn select_transaction_details_by_txid(
        &self,
        txid: &[u8],
    ) -> Result<Option<TransactionDetails>, Error> {
        let mut statement = self.connection.prepare_cached("SELECT transaction_details.timestamp, transaction_details.received, transaction_details.sent, transaction_details.fee, transaction_details.height, transactions.raw_tx FROM transaction_details, transactions WHERE transaction_details.txid=transactions.txid AND transaction_details.txid=:txid")?;
        let mut rows = statement.query(named_params! { ":txid": txid })?;

        match rows.next()? {
            Some(row) => {
                let timestamp: Option<u64> = row.get(0)?;
                let received: u64 = row.get(1)?;
                let sent: u64 = row.get(2)?;
                let fee: Option<u64> = row.get(3)?;
                let height: Option<u32> = row.get(4)?;

                let raw_tx: Option<Vec<u8>> = row.get(5)?;
                let tx: Option<Transaction> = match raw_tx {
                    Some(raw_tx) => {
                        let tx: Transaction = deserialize(&raw_tx)?;
                        Some(tx)
                    }
                    None => None,
                };

                let confirmation_time = match (height, timestamp) {
                    (Some(height), Some(timestamp)) => Some(BlockTime { height, timestamp }),
                    _ => None,
                };

                Ok(Some(TransactionDetails {
                    transaction: tx,
                    txid: deserialize(txid)?,
                    received,
                    sent,
                    fee,
                    confirmation_time,
                }))
            }
            None => Ok(None),
        }
    }

    fn select_last_derivation_index_by_keychain(
        &self,
        keychain: String,
    ) -> Result<Option<u32>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT value FROM last_derivation_indices WHERE keychain=:keychain")?;
        let mut rows = statement.query(named_params! {":keychain": keychain})?;
        match rows.next()? {
            Some(row) => {
                let value: u32 = row.get(0)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn select_sync_time(&self) -> Result<Option<SyncTime>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT height, timestamp FROM sync_time WHERE id = 0")?;
        let mut rows = statement.query([])?;

        if let Some(row) = rows.next()? {
            Ok(Some(SyncTime {
                block_time: BlockTime {
                    height: row.get(0)?,
                    timestamp: row.get(1)?,
                },
            }))
        } else {
            Ok(None)
        }
    }

    fn select_checksum_by_keychain(&self, keychain: String) -> Result<Option<Vec<u8>>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT checksum FROM checksums WHERE keychain=:keychain")?;
        let mut rows = statement.query(named_params! {":keychain": keychain})?;

        match rows.next()? {
            Some(row) => {
                let checksum: Vec<u8> = row.get(0)?;
                Ok(Some(checksum))
            }
            None => Ok(None),
        }
    }

    fn delete_script_pubkey_by_path(&self, keychain: String, child: u32) -> Result<(), Error> {
        let mut statement = self.connection.prepare_cached(
            "DELETE FROM script_pubkeys WHERE keychain=:keychain AND child=:child",
        )?;
        statement.execute(named_params! {
            ":keychain": keychain,
            ":child": child
        })?;

        Ok(())
    }

    fn delete_script_pubkey_by_script(&self, script: &[u8]) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM script_pubkeys WHERE script=:script")?;
        statement.execute(named_params! {
            ":script": script
        })?;

        Ok(())
    }

    fn delete_utxo_by_outpoint(&self, txid: &[u8], vout: u32) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM utxos WHERE txid=:txid AND vout=:vout")?;
        statement.execute(named_params! {
            ":txid": txid,
            ":vout": vout
        })?;

        Ok(())
    }

    fn delete_transaction_by_txid(&self, txid: &[u8]) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM transactions WHERE txid=:txid")?;
        statement.execute(named_params! {":txid": txid})?;
        Ok(())
    }

    fn delete_transaction_details_by_txid(&self, txid: &[u8]) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM transaction_details WHERE txid=:txid")?;
        statement.execute(named_params! {":txid": txid})?;
        Ok(())
    }

    fn delete_last_derivation_index_by_keychain(&self, keychain: String) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM last_derivation_indices WHERE keychain=:keychain")?;
        statement.execute(named_params! {
            ":keychain": &keychain
        })?;

        Ok(())
    }

    fn delete_sync_time(&self) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM sync_time WHERE id = 0")?;
        statement.execute([])?;
        Ok(())
    }
}

impl BatchOperations for SqliteDatabase {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<(), Error> {
        let keychain = serde_json::to_string(&keychain)?;
        self.insert_script_pubkey(keychain, child, script.as_bytes())?;
        Ok(())
    }

    fn set_utxo(&mut self, utxo: &LocalUtxo) -> Result<(), Error> {
        self.insert_utxo(
            utxo.txout.value,
            serde_json::to_string(&utxo.keychain)?,
            utxo.outpoint.vout,
            &utxo.outpoint.txid,
            utxo.txout.script_pubkey.as_bytes(),
            utxo.is_spent,
        )?;
        Ok(())
    }

    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
        match self.select_transaction_by_txid(&transaction.txid())? {
            Some(_) => {
                self.update_transaction(&transaction.txid(), &serialize(transaction))?;
            }
            None => {
                self.insert_transaction(&transaction.txid(), &serialize(transaction))?;
            }
        }
        Ok(())
    }

    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error> {
        match self.select_transaction_details_by_txid(&transaction.txid)? {
            Some(_) => {
                self.update_transaction_details(transaction)?;
            }
            None => {
                self.insert_transaction_details(transaction)?;
            }
        }

        if let Some(tx) = &transaction.transaction {
            self.set_raw_tx(tx)?;
        }

        Ok(())
    }

    fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), Error> {
        self.update_last_derivation_index(serde_json::to_string(&keychain)?, value)?;
        Ok(())
    }

    fn set_sync_time(&mut self, ct: SyncTime) -> Result<(), Error> {
        self.update_sync_time(ct)?;
        Ok(())
    }

    fn del_script_pubkey_from_path(
        &mut self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<Script>, Error> {
        let keychain = serde_json::to_string(&keychain)?;
        let script = self.select_script_pubkey_by_path(keychain.clone(), child)?;
        match script {
            Some(script) => {
                self.delete_script_pubkey_by_path(keychain, child)?;
                Ok(Some(script))
            }
            None => Ok(None),
        }
    }

    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        match self.select_script_pubkey_by_script(script.as_bytes())? {
            Some((keychain, child)) => {
                self.delete_script_pubkey_by_script(script.as_bytes())?;
                Ok(Some((keychain, child)))
            }
            None => Ok(None),
        }
    }

    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        match self.select_utxo_by_outpoint(&outpoint.txid, outpoint.vout)? {
            Some(local_utxo) => {
                self.delete_utxo_by_outpoint(&outpoint.txid, outpoint.vout)?;
                Ok(Some(local_utxo))
            }
            None => Ok(None),
        }
    }

    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        match self.select_transaction_by_txid(txid)? {
            Some(tx) => {
                self.delete_transaction_by_txid(txid)?;
                Ok(Some(tx))
            }
            None => Ok(None),
        }
    }

    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, Error> {
        match self.select_transaction_details_by_txid(txid)? {
            Some(transaction_details) => {
                self.delete_transaction_details_by_txid(txid)?;

                if include_raw {
                    self.delete_transaction_by_txid(txid)?;
                }
                Ok(Some(transaction_details))
            }
            None => Ok(None),
        }
    }

    fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        let keychain = serde_json::to_string(&keychain)?;
        match self.select_last_derivation_index_by_keychain(keychain.clone())? {
            Some(value) => {
                self.delete_last_derivation_index_by_keychain(keychain)?;

                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn del_sync_time(&mut self) -> Result<Option<SyncTime>, Error> {
        match self.select_sync_time()? {
            Some(value) => {
                self.delete_sync_time()?;

                Ok(Some(value))
            }
            None => Ok(None),
        }
    }
}

impl Database for SqliteDatabase {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        keychain: KeychainKind,
        bytes: B,
    ) -> Result<(), Error> {
        let keychain = serde_json::to_string(&keychain)?;

        match self.select_checksum_by_keychain(keychain.clone())? {
            Some(checksum) => {
                if checksum == bytes.as_ref().to_vec() {
                    Ok(())
                } else {
                    Err(Error::ChecksumMismatch)
                }
            }
            None => {
                self.insert_checksum(keychain, bytes.as_ref())?;
                Ok(())
            }
        }
    }

    fn iter_script_pubkeys(&self, keychain: Option<KeychainKind>) -> Result<Vec<Script>, Error> {
        match keychain {
            Some(keychain) => {
                let keychain = serde_json::to_string(&keychain)?;
                self.select_script_pubkeys_by_keychain(keychain)
            }
            None => self.select_script_pubkeys(),
        }
    }

    fn iter_utxos(&self) -> Result<Vec<LocalUtxo>, Error> {
        self.select_utxos()
    }

    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error> {
        self.select_transactions()
    }

    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        match include_raw {
            true => self.select_transaction_details_with_raw(),
            false => self.select_transaction_details(),
        }
    }

    fn get_script_pubkey_from_path(
        &self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<Script>, Error> {
        let keychain = serde_json::to_string(&keychain)?;
        match self.select_script_pubkey_by_path(keychain, child)? {
            Some(script) => Ok(Some(script)),
            None => Ok(None),
        }
    }

    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        match self.select_script_pubkey_by_script(script.as_bytes())? {
            Some((keychain, child)) => Ok(Some((keychain, child))),
            None => Ok(None),
        }
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        self.select_utxo_by_outpoint(&outpoint.txid, outpoint.vout)
    }

    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        match self.select_transaction_by_txid(txid)? {
            Some(tx) => Ok(Some(tx)),
            None => Ok(None),
        }
    }

    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        match self.select_transaction_details_by_txid(txid)? {
            Some(mut transaction_details) => {
                if !include_raw {
                    transaction_details.transaction = None;
                }
                Ok(Some(transaction_details))
            }
            None => Ok(None),
        }
    }

    fn get_last_index(&self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        let keychain = serde_json::to_string(&keychain)?;
        let value = self.select_last_derivation_index_by_keychain(keychain)?;
        Ok(value)
    }

    fn get_sync_time(&self) -> Result<Option<SyncTime>, Error> {
        self.select_sync_time()
    }

    fn increment_last_index(&mut self, keychain: KeychainKind) -> Result<u32, Error> {
        let keychain_string = serde_json::to_string(&keychain)?;
        match self.get_last_index(keychain)? {
            Some(value) => {
                self.update_last_derivation_index(keychain_string, value + 1)?;
                Ok(value + 1)
            }
            None => {
                self.insert_last_derivation_index(keychain_string, 0)?;
                Ok(0)
            }
        }
    }

    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

impl BatchDatabase for SqliteDatabase {
    type Batch = SqliteDatabase;

    fn begin_batch(&self) -> Self::Batch {
        let db = SqliteDatabase::new(self.path.clone());
        db.connection.execute("BEGIN TRANSACTION", []).unwrap();
        db
    }

    fn commit_batch(&mut self, batch: Self::Batch) -> Result<(), Error> {
        batch.connection.execute("COMMIT TRANSACTION", [])?;
        Ok(())
    }
}

pub fn get_connection(path: &str) -> Result<Connection, Error> {
    let connection = Connection::open(path)?;
    migrate(&connection)?;
    Ok(connection)
}

pub fn get_schema_version(conn: &Connection) -> rusqlite::Result<i32> {
    let statement = conn.prepare_cached("SELECT version FROM version");
    match statement {
        Err(rusqlite::Error::SqliteFailure(e, Some(msg))) => {
            if msg == "no such table: version" {
                Ok(0)
            } else {
                Err(rusqlite::Error::SqliteFailure(e, Some(msg)))
            }
        }
        Ok(mut stmt) => {
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => {
                    let version: i32 = row.get(0)?;
                    Ok(version)
                }
                None => Ok(0),
            }
        }
        _ => Ok(0),
    }
}

pub fn set_schema_version(conn: &Connection, version: i32) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE version SET version=:version",
        named_params! {":version": version},
    )
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version = get_schema_version(conn)?;
    let stmts = &MIGRATIONS[(version as usize)..];
    let mut i: i32 = version;

    if version == MIGRATIONS.len() as i32 {
        log::info!("db up to date, no migration needed");
        return Ok(());
    }

    for stmt in stmts {
        let res = conn.execute(stmt, []);
        if res.is_err() {
            println!("migration failed on:\n{}\n{:?}", stmt, res);
            break;
        }

        i += 1;
    }

    set_schema_version(conn, i)?;

    Ok(())
}

#[cfg(test)]
pub mod test {
    use crate::database::SqliteDatabase;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn get_database() -> SqliteDatabase {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let mut dir = std::env::temp_dir();
        dir.push(format!("bdk_{}", time.as_nanos()));
        SqliteDatabase::new(String::from(dir.to_str().unwrap()))
    }

    #[test]
    fn test_script_pubkey() {
        crate::database::test::test_script_pubkey(get_database());
    }

    #[test]
    fn test_batch_script_pubkey() {
        crate::database::test::test_batch_script_pubkey(get_database());
    }

    #[test]
    fn test_iter_script_pubkey() {
        crate::database::test::test_iter_script_pubkey(get_database());
    }

    #[test]
    fn test_del_script_pubkey() {
        crate::database::test::test_del_script_pubkey(get_database());
    }

    #[test]
    fn test_utxo() {
        crate::database::test::test_utxo(get_database());
    }

    #[test]
    fn test_raw_tx() {
        crate::database::test::test_raw_tx(get_database());
    }

    #[test]
    fn test_tx() {
        crate::database::test::test_tx(get_database());
    }

    #[test]
    fn test_last_index() {
        crate::database::test::test_last_index(get_database());
    }

    #[test]
    fn test_sync_time() {
        crate::database::test::test_sync_time(get_database());
    }
}
