/* This file is part of DarkFi (https://dark.fi)
 *
 * Copyright (C) 2020-2023 Dyne.org foundation
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of the
 * License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Affero General Public License for more details.
 *
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use darkfi_sdk::crypto::{
    pedersen::pedersen_commitment_u64, poseidon_hash, Keypair, MerkleNode, Nullifier, PublicKey,
    SecretKey,
};
use darkfi_serial::{SerialDecodable, SerialEncodable};
use halo2_proofs::circuit::Value;
use incrementalmerkletree::Hashable;
use log::debug;
use pasta_curves::{
    arithmetic::CurveAffine,
    group::{ff::Field, Curve},
    pallas,
};
use rand::rngs::OsRng;

use darkfi::{
    crypto::Proof,
    zk::vm::{Witness, ZkCircuit},
};

use crate::{
    contract::{
        dao::{
            mint::wallet::DaoParams,
            propose::wallet::Proposal,
            vote::validate::{CallData, Header, Input},
            CONTRACT_ID,
        },
        money,
    },
    note,
    util::{FuncCall, ZkContractInfo, ZkContractTable},
};

#[derive(SerialEncodable, SerialDecodable)]
pub struct Note {
    pub vote: Vote,
    pub vote_value: u64,
    pub vote_value_blind: pallas::Scalar,
}

#[derive(SerialEncodable, SerialDecodable)]
pub struct Vote {
    pub vote_option: bool,
    pub vote_option_blind: pallas::Scalar,
}

pub struct BuilderInput {
    pub secret: SecretKey,
    pub note: money::transfer::wallet::Note,
    pub leaf_position: incrementalmerkletree::Position,
    pub merkle_path: Vec<MerkleNode>,
    pub signature_secret: SecretKey,
}

// TODO: should be token locking voting?
// Inside ZKproof, check proposal is correct.
pub struct Builder {
    pub inputs: Vec<BuilderInput>,
    pub vote: Vote,
    pub vote_keypair: Keypair,
    pub proposal: Proposal,
    pub dao: DaoParams,
}

impl Builder {
    pub fn build(self, zk_bins: &ZkContractTable) -> FuncCall {
        debug!(target: "dao_contract::vote::wallet::Builder", "build()");
        let mut proofs = vec![];

        let gov_token_blind = pallas::Base::random(&mut OsRng);

        let mut inputs = vec![];
        let mut vote_value = 0;
        let mut vote_value_blind = pallas::Scalar::from(0);

        for input in self.inputs {
            let value_blind = pallas::Scalar::random(&mut OsRng);

            vote_value += input.note.value;
            vote_value_blind += value_blind;

            let signature_public = PublicKey::from_secret(input.signature_secret);

            let zk_info = zk_bins.lookup(&"dao-vote-burn".to_string()).unwrap();

            let zk_info = if let ZkContractInfo::Binary(info) = zk_info {
                info
            } else {
                panic!("Not binary info")
            };
            let zk_bin = zk_info.bincode.clone();

            // Note from the previous output
            let note = input.note;
            let leaf_pos: u64 = input.leaf_position.into();

            let prover_witnesses = vec![
                Witness::Base(Value::known(input.secret.inner())),
                Witness::Base(Value::known(note.serial)),
                Witness::Base(Value::known(pallas::Base::from(0))),
                Witness::Base(Value::known(pallas::Base::from(0))),
                Witness::Base(Value::known(pallas::Base::from(note.value))),
                Witness::Base(Value::known(note.token_id.inner())),
                Witness::Base(Value::known(note.coin_blind)),
                Witness::Scalar(Value::known(vote_value_blind)),
                Witness::Base(Value::known(gov_token_blind)),
                Witness::Uint32(Value::known(leaf_pos.try_into().unwrap())),
                Witness::MerklePath(Value::known(input.merkle_path.clone().try_into().unwrap())),
                Witness::Base(Value::known(input.signature_secret.inner())),
            ];

            let public_key = PublicKey::from_secret(input.secret);
            let (pub_x, pub_y) = public_key.xy();

            let coin = poseidon_hash::<8>([
                pub_x,
                pub_y,
                pallas::Base::from(note.value),
                note.token_id.inner(),
                note.serial,
                pallas::Base::from(0),
                pallas::Base::from(0),
                note.coin_blind,
            ]);

            let merkle_root = {
                let position: u64 = input.leaf_position.into();
                let mut current = MerkleNode::from(coin);
                for (level, sibling) in input.merkle_path.iter().enumerate() {
                    let level = level as u8;
                    current = if position & (1 << level) == 0 {
                        MerkleNode::combine(level.into(), &current, sibling)
                    } else {
                        MerkleNode::combine(level.into(), sibling, &current)
                    };
                }
                current
            };

            let token_commit = poseidon_hash::<2>([note.token_id.inner(), gov_token_blind]);
            assert_eq!(self.dao.gov_token_id, note.token_id);

            let nullifier = poseidon_hash::<2>([input.secret.inner(), note.serial]);

            let vote_commit = pedersen_commitment_u64(note.value, vote_value_blind);
            let vote_commit_coords = vote_commit.to_affine().coordinates().unwrap();

            let (sig_x, sig_y) = signature_public.xy();

            let public_inputs = vec![
                nullifier,
                *vote_commit_coords.x(),
                *vote_commit_coords.y(),
                token_commit,
                merkle_root.inner(),
                sig_x,
                sig_y,
            ];

            let circuit = ZkCircuit::new(prover_witnesses, zk_bin);
            let proving_key = &zk_info.proving_key;
            debug!(target: "dao_contract::vote::wallet::Builder", "input_proof Proof::create()");
            let input_proof = Proof::create(proving_key, &[circuit], &public_inputs, &mut OsRng)
                .expect("DAO::vote() proving error!");
            proofs.push(input_proof);

            let input = Input {
                nullifier: Nullifier::from(nullifier),
                vote_commit,
                merkle_root,
                signature_public,
            };
            inputs.push(input);
        }

        let token_commit = poseidon_hash::<2>([self.dao.gov_token_id.inner(), gov_token_blind]);

        let (proposal_dest_x, proposal_dest_y) = self.proposal.dest.xy();

        let proposal_amount = pallas::Base::from(self.proposal.amount);

        let dao_proposer_limit = pallas::Base::from(self.dao.proposer_limit);
        let dao_quorum = pallas::Base::from(self.dao.quorum);
        let dao_approval_ratio_quot = pallas::Base::from(self.dao.approval_ratio_quot);
        let dao_approval_ratio_base = pallas::Base::from(self.dao.approval_ratio_base);

        let (dao_pub_x, dao_pub_y) = self.dao.public_key.xy();

        let dao_bulla = poseidon_hash::<8>([
            dao_proposer_limit,
            dao_quorum,
            dao_approval_ratio_quot,
            dao_approval_ratio_base,
            self.dao.gov_token_id.inner(),
            dao_pub_x,
            dao_pub_y,
            self.dao.bulla_blind,
        ]);

        let proposal_bulla = poseidon_hash::<8>([
            proposal_dest_x,
            proposal_dest_y,
            proposal_amount,
            self.proposal.serial,
            self.proposal.token_id.inner(),
            dao_bulla,
            self.proposal.blind,
            // @tmp-workaround
            self.proposal.blind,
        ]);

        let vote_option = self.vote.vote_option as u64;
        assert!(vote_option == 0 || vote_option == 1);

        let yes_vote_commit =
            pedersen_commitment_u64(vote_option * vote_value, self.vote.vote_option_blind);
        let yes_vote_commit_coords = yes_vote_commit.to_affine().coordinates().unwrap();

        let all_vote_commit = pedersen_commitment_u64(vote_value, vote_value_blind);
        let all_vote_commit_coords = all_vote_commit.to_affine().coordinates().unwrap();

        let zk_info = zk_bins.lookup(&"dao-vote-main".to_string()).unwrap();
        let zk_info = if let ZkContractInfo::Binary(info) = zk_info {
            info
        } else {
            panic!("Not binary info")
        };
        let zk_bin = zk_info.bincode.clone();

        let prover_witnesses = vec![
            // proposal params
            Witness::Base(Value::known(proposal_dest_x)),
            Witness::Base(Value::known(proposal_dest_y)),
            Witness::Base(Value::known(proposal_amount)),
            Witness::Base(Value::known(self.proposal.serial)),
            Witness::Base(Value::known(self.proposal.token_id.inner())),
            Witness::Base(Value::known(self.proposal.blind)),
            // DAO params
            Witness::Base(Value::known(dao_proposer_limit)),
            Witness::Base(Value::known(dao_quorum)),
            Witness::Base(Value::known(dao_approval_ratio_quot)),
            Witness::Base(Value::known(dao_approval_ratio_base)),
            Witness::Base(Value::known(self.dao.gov_token_id.inner())),
            Witness::Base(Value::known(dao_pub_x)),
            Witness::Base(Value::known(dao_pub_y)),
            Witness::Base(Value::known(self.dao.bulla_blind)),
            // Vote
            Witness::Base(Value::known(pallas::Base::from(vote_option))),
            Witness::Scalar(Value::known(self.vote.vote_option_blind)),
            // Total number of gov tokens allocated
            Witness::Base(Value::known(pallas::Base::from(vote_value))),
            Witness::Scalar(Value::known(vote_value_blind)),
            // gov token
            Witness::Base(Value::known(gov_token_blind)),
        ];

        let public_inputs = vec![
            token_commit,
            proposal_bulla,
            // this should be a value commit??
            *yes_vote_commit_coords.x(),
            *yes_vote_commit_coords.y(),
            *all_vote_commit_coords.x(),
            *all_vote_commit_coords.y(),
        ];

        let circuit = ZkCircuit::new(prover_witnesses, zk_bin);

        let proving_key = &zk_info.proving_key;
        debug!(target: "dao_contract::vote::wallet::Builder", "main_proof = Proof::create()");
        let main_proof = Proof::create(proving_key, &[circuit], &public_inputs, &mut OsRng)
            .expect("DAO::vote() proving error!");
        proofs.push(main_proof);

        let note = Note { vote: self.vote, vote_value, vote_value_blind };
        let enc_note = note::encrypt(&note, &self.vote_keypair.public).unwrap();

        let header = Header { token_commit, proposal_bulla, yes_vote_commit, enc_note };

        let call_data = CallData { header, inputs };

        FuncCall {
            contract_id: *CONTRACT_ID,
            func_id: *super::FUNC_ID,
            call_data: Box::new(call_data),
            proofs,
        }
    }
}
