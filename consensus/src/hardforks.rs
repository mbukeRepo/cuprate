use std::ops::Range;

use monero_serai::block::BlockHeader;
use tower::ServiceExt;
use tracing::instrument;

use cuprate_common::{BlockID, Network};

use crate::{Database, DatabaseRequest, DatabaseResponse, Error};

//http://localhost:3000/consensus_rules/hardforks.html#window-size
const DEFAULT_WINDOW_SIZE: u64 = 10080; // supermajority window check length - a week

/// An identifier for every hard-fork Monero has had.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
#[repr(u8)]
pub enum HardFork {
    V1 = 1,
    V2,
    V3,
    V4,
    V5,
    V6,
    V7,
    V8,
    V9,
    V10,
    V11,
    V12,
    V13,
    V14,
    V15,
    V16,
}

impl HardFork {
    /// Returns the hard-fork for a blocks `major_version` field.
    ///
    /// http://**/consensus_rules/hardforks.html#blocks-version-and-vote
    pub fn from_version(version: &u8) -> Result<HardFork, Error> {
        Ok(match version {
            1 => HardFork::V1,
            2 => HardFork::V2,
            3 => HardFork::V3,
            4 => HardFork::V4,
            5 => HardFork::V5,
            6 => HardFork::V6,
            7 => HardFork::V7,
            8 => HardFork::V8,
            9 => HardFork::V9,
            10 => HardFork::V10,
            11 => HardFork::V11,
            12 => HardFork::V12,
            13 => HardFork::V13,
            14 => HardFork::V14,
            15 => HardFork::V15,
            16 => HardFork::V16,
            _ => {
                return Err(Error::InvalidHardForkVersion(
                    "Version is not a known hard fork",
                ))
            }
        })
    }

    /// Returns the hard-fork for a blocks `minor_version` (vote) field.
    ///
    /// http://**/consensus_rules/hardforks.html#blocks-version-and-vote
    pub fn from_vote(vote: &u8) -> HardFork {
        if *vote == 0 {
            // A vote of 0 is interpreted as 1 as that's what Monero used to default to.
            return HardFork::V1;
        }
        // This must default to the latest hard-fork!
        Self::from_version(vote).unwrap_or(HardFork::V16)
    }

    /// Returns the next hard-fork.
    pub fn next_fork(&self) -> Option<HardFork> {
        match self {
            HardFork::V1 => Some(HardFork::V2),
            HardFork::V2 => Some(HardFork::V3),
            HardFork::V3 => Some(HardFork::V4),
            HardFork::V4 => Some(HardFork::V5),
            HardFork::V5 => Some(HardFork::V6),
            HardFork::V6 => Some(HardFork::V7),
            HardFork::V7 => Some(HardFork::V8),
            HardFork::V8 => Some(HardFork::V9),
            HardFork::V9 => Some(HardFork::V10),
            HardFork::V10 => Some(HardFork::V11),
            HardFork::V11 => Some(HardFork::V12),
            HardFork::V12 => Some(HardFork::V13),
            HardFork::V13 => Some(HardFork::V14),
            HardFork::V14 => Some(HardFork::V15),
            HardFork::V15 => Some(HardFork::V16),
            HardFork::V16 => None,
        }
    }

    /// Returns the threshold of this fork.
    pub fn fork_threshold(&self, _: &Network) -> u64 {
        0
    }

    /// Returns the votes needed for this fork.
    pub fn votes_needed(&self, network: &Network, window: u64) -> u64 {
        (self.fork_threshold(network) * window + 99) / 100
    }

    /// Returns the minimum height this fork will activate at
    pub fn fork_height(&self, network: &Network) -> u64 {
        match network {
            Network::Mainnet => self.mainnet_fork_height(),
            Network::Stagenet => self.stagenet_fork_height(),
            Network::Testnet => self.testnet_fork_height(),
        }
    }

    fn stagenet_fork_height(&self) -> u64 {
        todo!()
    }

    fn testnet_fork_height(&self) -> u64 {
        todo!()
    }

    fn mainnet_fork_height(&self) -> u64 {
        match self {
            HardFork::V1 => 0, // Monero core has this as 1, which is strange
            HardFork::V2 => 1009827,
            HardFork::V3 => 1141317,
            HardFork::V4 => 1220516,
            HardFork::V5 => 1288616,
            HardFork::V6 => 1400000,
            HardFork::V7 => 1546000,
            HardFork::V8 => 1685555,
            HardFork::V9 => 1686275,
            HardFork::V10 => 1788000,
            HardFork::V11 => 1788720,
            HardFork::V12 => 1978433,
            HardFork::V13 => 2210000,
            HardFork::V14 => 2210720,
            HardFork::V15 => 2688888,
            HardFork::V16 => 2689608,
        }
    }
}

/// A struct holding the current voting state of the blockchain.
#[derive(Debug, Default)]
struct HFVotes {
    votes: [u64; 16],
}

impl HFVotes {
    /// Add votes for a hard-fork
    pub fn add_votes_for_hf(&mut self, hf: &HardFork, votes: u64) {
        self.votes[*hf as usize - 1] += votes;
    }

    /// Add a vote for a hard-fork.
    pub fn add_vote_for_hf(&mut self, hf: &HardFork) {
        self.add_votes_for_hf(hf, 1)
    }

    /// Remove a vote for a hard-fork.
    pub fn remove_vote_for_hf(&mut self, hf: &HardFork) {
        self.votes[*hf as usize - 1] -= 1;
    }

    /// Returns the total votes for a hard-fork.
    ///
    /// http://localhost:3000/consensus_rules/hardforks.html#accepting-a-fork
    pub fn get_votes_for_hf(&self, hf: &HardFork) -> u64 {
        self.votes[*hf as usize - 1..].iter().sum()
    }

    /// Returns the total amount of votes being tracked
    pub fn total_votes(&self) -> u64 {
        self.votes.iter().sum()
    }
}

/// Configuration for hard-forks.
///
#[derive(Debug)]
pub struct HardForkConfig {
    /// The network we are on.
    network: Network,
    /// The amount of votes we are taking into account to decide on a fork activation.
    window: u64,
}

impl Default for HardForkConfig {
    fn default() -> Self {
        Self {
            network: Network::Mainnet,
            window: 3, //DEFAULT_WINDOW_SIZE,
        }
    }
}

/// A struct that keeps track of the current hard-fork and current votes.
#[derive(Debug)]
pub struct HardForks {
    current_hardfork: HardFork,
    next_hardfork: Option<HardFork>,

    config: HardForkConfig,
    votes: HFVotes,

    last_height: u64,
}

impl HardForks {
    pub async fn init<D>(config: HardForkConfig, database: &mut D) -> Result<Self, Error>
    where
        D: Database,
    {
        let DatabaseResponse::ChainHeight(chain_height) = database
             .ready()
             .await?
             .call(DatabaseRequest::ChainHeight)
             .await? else {
             panic!("Database sent incorrect response")
         };

        let block_heights = if chain_height > config.window {
            chain_height - config.window..chain_height
        } else {
            0..chain_height
        };

        let votes = get_votes_in_range(database, block_heights).await?;

        if chain_height > config.window {
            assert_eq!(votes.total_votes(), config.window)
        }

        let latest_header = get_block_header(database, chain_height - 1).await?;

        let current_hardfork = HardFork::from_version(&latest_header.major_version)
            .expect("Invalid major version in stored block");

        let next_hardfork = current_hardfork.next_fork();

        let mut hfs = HardForks {
            config,
            current_hardfork,
            next_hardfork,
            votes,
            last_height: chain_height - 1,
        };

        // chain_height = height + 1
        hfs.check_set_new_hf(chain_height);

        Ok(hfs)
    }

    pub fn check_block_version_vote(&self, version: &HardFork, vote: &HardFork) -> bool {
        &self.current_hardfork == version && vote >= &self.current_hardfork
    }

    pub async fn new_block<D: Database>(&mut self, vote: HardFork, height: u64, database: &mut D) {
        assert_eq!(self.last_height + 1, height);
        self.last_height += 1;

        self.votes.add_vote_for_hf(&vote);

        for offset in self.config.window..self.votes.total_votes() {
            let header = get_block_header(database, height - offset)
                .await
                .expect("Error retrieving block we should have in database");
            self.votes
                .remove_vote_for_hf(&HardFork::from_vote(&header.minor_version));
        }

        if height > self.config.window {
            assert_eq!(self.votes.total_votes(), self.config.window);
        }

        self.check_set_new_hf(height + 1)
    }

    fn check_set_new_hf(&mut self, height: u64) {
        while let Some(new_hf) = self.next_hardfork {
            if height >= new_hf.fork_height(&self.config.network)
                && self.votes.get_votes_for_hf(&new_hf)
                    >= new_hf.votes_needed(&self.config.network, self.config.window)
            {
                self.set_hf(new_hf);
            } else {
                return;
            }
        }
    }

    fn set_hf(&mut self, new_hf: HardFork) {
        self.next_hardfork = new_hf.next_fork();
        self.current_hardfork = new_hf;
    }
}

#[instrument(skip(database))]
async fn get_votes_in_range<D: Database>(
    database: &mut D,
    block_heights: Range<u64>,
) -> Result<HFVotes, Error> {
    let mut votes = HFVotes::default();

    for height in block_heights {
        let header = get_block_header(database, height).await?;

        let vote = HardFork::from_vote(&header.minor_version);

        tracing::info!("Block vote for height: {} = {:?}", height, vote);

        votes.add_vote_for_hf(&HardFork::from_vote(&header.minor_version));
    }

    Ok(votes)
}

async fn get_block_header<D: Database>(
    database: &mut D,
    block_id: impl Into<BlockID>,
) -> Result<BlockHeader, Error> {
    let DatabaseResponse::BlockHeader(header) = database
        .oneshot(DatabaseRequest::BlockHeader(block_id.into()))
        .await? else {
        panic!("Database sent incorrect response for block header request")
    };
    Ok(header)
}

#[test]
fn to_from_hf() {
    let hf = HardFork::V1 as u8;

    assert_eq!(hf, 1)
}
