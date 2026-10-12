/*!
 * Keep stats for the entire run
 */

use std::time::Duration;

use anyhow::Result;

use super::level::LevelDefinition;

pub struct Stats {
    pub playtime: Duration,
    pub levels_beaten: usize,
    pub coins_collected: usize,
    pub coins_total: usize,
    pub deaths: usize,
}

impl Stats {
    pub fn new(start_level: usize, levels: &[&str]) -> Result<Self> {
        // figure out how many coins there are total for all levels
        let coins_total = levels[start_level..]
            .iter()
            .map(|source| LevelDefinition::parse(source))
            .try_fold(0, |total, level| {
                level.map(|level| total + level.coins.len())
            })?;

        Ok(Self {
            playtime: Duration::ZERO,
            levels_beaten: 0,
            coins_collected: 0,
            coins_total,
            deaths: 0,
        })
    }

    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("total playtime: {}", self.format_playtime()),
            format!("levels beaten: {}", self.levels_beaten),
            format!("coins collected: {}", self.format_coins()),
            format!("deaths: {}", self.deaths),
        ];
        if self.is_true_ending() {
            lines.push("True Ending!".to_string());
        }
        lines
    }

    fn is_true_ending(&self) -> bool {
        self.coins_collected == self.coins_total && self.deaths == 0
    }

    fn format_coins(&self) -> String {
        let smiley = if self.coins_collected == self.coins_total {
            ":)"
        } else {
            ":("
        };
        format!("{}/{} {}", self.coins_collected, self.coins_total, smiley)
    }

    fn format_playtime(&self) -> String {
        let seconds = self.playtime.as_secs();
        let hours = seconds / 60 / 60;
        let minutes = seconds / 60 % 60;
        let seconds = seconds % 60;
        let milliseconds = self.playtime.subsec_millis();

        format!("{hours}:{minutes:02}:{seconds:02}.{milliseconds:03}")
    }
}
