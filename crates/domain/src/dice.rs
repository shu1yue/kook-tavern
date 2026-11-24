use std::sync::OnceLock;

use rand::Rng;
use regex::Regex;

use crate::models::DomainError;

#[derive(Debug, Clone)]
pub struct DiceRollResult {
    pub expression: String,
    pub rolls: Vec<i32>,
    pub modifier: i32,
    pub total: i32,
    pub mode_note: Option<String>,
    pub reason: Option<String>,
}

impl DiceRollResult {
    pub fn format_short(&self) -> String {
        let rolls_str = self
            .rolls
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let mod_str = if self.modifier > 0 {
            format!(" + {}", self.modifier)
        } else if self.modifier < 0 {
            format!(" - {}", -self.modifier)
        } else {
            String::new()
        };
        let mode = self
            .mode_note
            .as_deref()
            .map(|m| format!("（{m}）"))
            .unwrap_or_default();
        let reason = self
            .reason
            .as_deref()
            .map(|r| format!("【{r}】"))
            .unwrap_or_default();

        format!(
            "🎲 {reason}{expr}{mode} = [{rolls_str}]{mod_str} → **{total}**",
            expr = self.expression,
            total = self.total
        )
    }
}

fn dice_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(\d*)d(\d+)([+-]\d+)?$").expect("valid dice regex"))
}

pub fn roll_dice(expr: &str, reason: Option<String>) -> Result<DiceRollResult, DomainError> {
    let mut rng = rand::thread_rng();
    roll_dice_with_rng(expr, reason, &mut rng)
}

pub fn roll_dice_with_rng<R: Rng + ?Sized>(
    expr: &str,
    reason: Option<String>,
    rng: &mut R,
) -> Result<DiceRollResult, DomainError> {
    let cleaned = expr.trim().to_ascii_lowercase();
    if cleaned.is_empty() {
        return Err(DomainError::InvalidDice(
            "请提供骰子表达式，例如 `d100`、`2d6+1`、`1d20`、`adv`（优势）".into(),
        ));
    }

    if matches!(cleaned.as_str(), "adv" | "优势") {
        let a = rng.gen_range(1..=20);
        let b = rng.gen_range(1..=20);
        return Ok(DiceRollResult {
            expression: "2d20kh1".to_string(),
            rolls: vec![a, b],
            modifier: 0,
            total: a.max(b),
            mode_note: Some("优势取高".to_string()),
            reason,
        });
    }

    if matches!(cleaned.as_str(), "dis" | "劣势") {
        let a = rng.gen_range(1..=20);
        let b = rng.gen_range(1..=20);
        return Ok(DiceRollResult {
            expression: "2d20kl1".to_string(),
            rolls: vec![a, b],
            modifier: 0,
            total: a.min(b),
            mode_note: Some("劣势取低".to_string()),
            reason,
        });
    }

    let caps = dice_regex().captures(&cleaned).ok_or_else(|| {
        DomainError::InvalidDice(format!(
            "无法识别 `{expr}`，支持格式如 `d100`、`2d6+2`、`1d20-1`、`adv`、`dis`"
        ))
    })?;

    let count_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let count: usize = if count_str.is_empty() {
        1
    } else {
        count_str
            .parse::<usize>()
            .map_err(|_| DomainError::InvalidDice(format!("骰子数量超出范围: {count_str}")))?
    };
    let sides: i32 = caps
        .get(2)
        .unwrap()
        .as_str()
        .parse::<i32>()
        .map_err(|_| DomainError::InvalidDice("骰子面数超出范围".into()))?;
    let modifier: i32 = match caps.get(3) {
        Some(m) => m
            .as_str()
            .parse::<i32>()
            .map_err(|_| DomainError::InvalidDice("修正值超出范围".into()))?,
        None => 0,
    };

    if !(1..=50).contains(&count) {
        return Err(DomainError::InvalidDice("单次掷骰数量必须在 1 到 50 之间".into()));
    }
    if !(2..=1000).contains(&sides) {
        return Err(DomainError::InvalidDice("骰子面数必须在 2 到 1000 之间".into()));
    }
    if !(-1000..=1000).contains(&modifier) {
        return Err(DomainError::InvalidDice("修正值必须在 -1000 到 1000 之间".into()));
    }

    let mut rolls = Vec::with_capacity(count);
    for _ in 0..count {
        rolls.push(rng.gen_range(1..=sides));
    }
    let total = rolls.iter().sum::<i32>() + modifier;

    Ok(DiceRollResult {
        expression: format!(
            "{count}d{sides}{}",
            if modifier > 0 {
                format!("+{modifier}")
            } else if modifier < 0 {
                format!("{modifier}")
            } else {
                String::new()
            }
        ),
        rolls,
        modifier,
        total,
        mode_note: None,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn rolls_standard_and_advantage() {
        let mut rng = StdRng::seed_from_u64(42);
        let res = roll_dice_with_rng("2d6+3", Some("撬锁".into()), &mut rng).unwrap();
        assert_eq!(res.rolls.len(), 2);
        assert_eq!(res.total, res.rolls.iter().sum::<i32>() + 3);
        assert!(res.format_short().contains("撬锁"));

        let adv = roll_dice_with_rng("adv", None, &mut rng).unwrap();
        assert_eq!(adv.total, *adv.rolls.iter().max().unwrap());
    }

    #[test]
    fn rejects_abusive_dice_counts() {
        let mut rng = StdRng::seed_from_u64(1);
        assert!(roll_dice_with_rng("9999d6", None, &mut rng).is_err());
        assert!(roll_dice_with_rng("1d1", None, &mut rng).is_err());
        assert!(roll_dice_with_rng("notdice", None, &mut rng).is_err());
    }
}
