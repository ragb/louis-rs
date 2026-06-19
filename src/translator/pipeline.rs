use unicode_normalization::char::compose;

use crate::{
    Direction,
    parser::{AnchoredRule, HasDirection, Rule},
    translator::{
        CharacterDefinition, ResolvedTranslation, TranslationError, TranslationStage,
        table::{TableContext, multipass::MultipassTable, primary::PrimaryTable},
    },
};

/// Precompose base + combining-mark sequences (`a` + U+0300 → `à`), but only
/// when the result is a character the table defines. Undefined combinations
/// stay decomposed so the mark can translate on its own (as in IPA, where
/// diacritics get their own cells); this is why we compose against the table
/// instead of applying full NFC.
fn precompose(input: &str, defined: &CharacterDefinition) -> String {
    let mut result = String::with_capacity(input.len());
    let mut starter: Option<char> = None;
    for c in input.chars() {
        match starter {
            Some(s) => match compose(s, c) {
                Some(composed) if defined.contains(composed) => starter = Some(composed),
                _ => {
                    result.push(s);
                    starter = Some(c);
                }
            },
            None => starter = Some(c),
        }
    }
    if let Some(s) = starter {
        result.push(s);
    }
    result
}

#[derive(Debug)]
pub enum Transformation {
    Pre(MultipassTable),
    Primary(PrimaryTable),
    Post(MultipassTable),
}

impl Transformation {
    pub fn trace(&self, input: &str) -> Vec<ResolvedTranslation> {
        match self {
            Transformation::Pre(t) => t.trace(input),
            Transformation::Primary(t) => t.trace(input),
            Transformation::Post(t) => t.trace(input),
        }
    }

    fn translate(&self, input: &str) -> String {
        match self {
            Transformation::Pre(t) => t.translate(input),
            Transformation::Primary(t) => t.translate(input),
            Transformation::Post(t) => t.translate(input),
        }
    }
}

#[derive(Debug)]
pub struct TranslationPipeline {
    steps: Vec<Transformation>,
    /// Characters defined by the table; used to decide which base + combining
    /// mark sequences to precompose before translation
    defined_characters: CharacterDefinition,
}

impl TranslationPipeline {
    pub fn compile(rules: &[AnchoredRule], direction: Direction) -> Result<Self, TranslationError> {
        let ctx = TableContext::compile(rules)?;
        let defined_characters = ctx.character_definitions().clone();
        let mut steps = Vec::new();

        // ignore rules that aren't meant for the given direction
        let rules: Vec<_> = rules
            .iter()
            .filter(|r| r.is_direction(direction))
            .cloned()
            .collect();

        let correct_rules: Vec<AnchoredRule> = rules
            .iter()
            .filter(|r| matches!(r.rule, Rule::Correct { .. }))
            .cloned()
            .collect();
        if !correct_rules.is_empty() {
            let transform =
                MultipassTable::compile(&correct_rules, direction, TranslationStage::Pre, &ctx)?;
            steps.push(Transformation::Pre(transform));
        }
        let context = TableContext::compile(rules.as_slice())?;
        let transform = PrimaryTable::compile(
            rules.as_slice(),
            direction,
            TranslationStage::Main,
            &context,
        )?;
        steps.push(Transformation::Primary(transform));
        let pass2_rules: Vec<AnchoredRule> = rules
            .iter()
            .filter(|r| matches!(r.rule, Rule::Pass2 { .. }))
            .cloned()
            .collect();
        if !pass2_rules.is_empty() {
            let transform =
                MultipassTable::compile(&pass2_rules, direction, TranslationStage::Post1, &ctx)?;
            steps.push(Transformation::Post(transform));
        }
        let pass3_rules: Vec<AnchoredRule> = rules
            .iter()
            .filter(|r| matches!(r.rule, Rule::Pass3 { .. }))
            .cloned()
            .collect();
        if !pass3_rules.is_empty() {
            let transform =
                MultipassTable::compile(&pass3_rules, direction, TranslationStage::Post2, &ctx)?;
            steps.push(Transformation::Post(transform));
        }
        let pass4_rules: Vec<AnchoredRule> = rules
            .iter()
            .filter(|r| matches!(r.rule, Rule::Pass4 { .. }))
            .cloned()
            .collect();
        if !pass4_rules.is_empty() {
            let transform =
                MultipassTable::compile(&pass4_rules, direction, TranslationStage::Post3, &ctx)?;
            steps.push(Transformation::Post(transform));
        }
        match direction {
            Direction::Forward => Ok(Self {
                steps,
                defined_characters,
            }),
            Direction::Backward => Ok(Self {
                steps: steps.into_iter().rev().collect(),
                defined_characters,
            }),
        }
    }

    pub fn trace(&self, input: &str) -> Vec<Vec<ResolvedTranslation>> {
        let mut input = precompose(input, &self.defined_characters);
        let mut result: Vec<Vec<ResolvedTranslation>> = Vec::new();
        for step in &self.steps {
            let translations = step.trace(&input);
            input = translations.iter().map(|t| t.output()).collect();
            result.push(translations);
        }
        result
    }

    pub fn translate(&self, input: &str) -> String {
        let mut result = precompose(input, &self.defined_characters);
        for step in &self.steps {
            result = step.translate(&result);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::parser::RuleParser;

    fn parse_rule(source: &str) -> AnchoredRule {
        RuleParser::new(source).rule().unwrap().into()
    }

    #[test]
    fn correct() {
        let rules = [
            parse_rule("always foo 123"),
            parse_rule("always bar 456"),
            parse_rule("noback correct \"baz\" \"bar\""),
            parse_rule("space \\s 0"),
        ];
        let pipeline = TranslationPipeline::compile(&rules, Direction::Forward).unwrap();
        assert_eq!(pipeline.translate("baz"), "⠸");
        assert_eq!(pipeline.translate("foobaz"), "⠇⠸");
        assert_eq!(pipeline.translate("foobar"), "⠇⠸");
        assert_eq!(pipeline.translate("  "), "⠀⠀");
        assert_eq!(pipeline.translate("🐂"), "⠳⠭⠂⠋⠲⠴⠆");
    }

    #[test]
    fn pass2() {
        let rules = [
            parse_rule("always foo 123"),
            parse_rule("always bar 456"),
            parse_rule("noback pass2 @123 @1"),
            parse_rule("space \\s 0"),
        ];
        let pipeline = TranslationPipeline::compile(&rules, Direction::Forward).unwrap();
        assert_eq!(pipeline.translate("foo"), "⠁");
        assert_eq!(pipeline.translate("foobar"), "⠁⠸");
        assert_eq!(pipeline.translate("  "), "⠀⠀");
        assert_eq!(pipeline.translate("🐂"), "⠳⠭⠂⠋⠲⠴⠆");
    }

    #[test]
    fn pass2_with_capture() {
        let rules = [
            parse_rule("lowercase o 135"),
            parse_rule("lowercase ύ 5-13456"),
            parse_rule("sign ΄ 5"),
            parse_rule("attribute accent ΄"),
            parse_rule("noback pass2 @135[%accent]@13456 *@136"),
        ];
        let pipeline = TranslationPipeline::compile(&rules, Direction::Forward).unwrap();
        assert_eq!(pipeline.translate("o"), "⠕");
        assert_eq!(pipeline.translate("oύ"), "⠕⠐⠥⠽");
    }

    #[test]
    fn pass3() {
        let rules = [
            parse_rule("always foo 123"),
            parse_rule("always bar 456"),
            parse_rule("noback pass2 @123 @78"),
            parse_rule("noback pass3 @78 @1"),
            parse_rule("space \\s 0"),
        ];
        let pipeline = TranslationPipeline::compile(&rules, Direction::Forward).unwrap();
        assert_eq!(pipeline.translate("foo"), "⠁");
        assert_eq!(pipeline.translate("foobar"), "⠁⠸");
        assert_eq!(pipeline.translate("  "), "⠀⠀");
        assert_eq!(pipeline.translate("🐂"), "⠳⠭⠂⠋⠲⠴⠆");
    }

    #[test]
    fn pass4() {
        let rules = [
            parse_rule("always foo 123"),
            parse_rule("always bar 456"),
            parse_rule("noback pass2 @123 @67"),
            parse_rule("noback pass3 @67 @78"),
            parse_rule("noback pass4 @78 @1"),
            parse_rule("space \\s 0"),
        ];
        let pipeline = TranslationPipeline::compile(&rules, Direction::Forward).unwrap();
        assert_eq!(pipeline.translate("foo"), "⠁");
        assert_eq!(pipeline.translate("foobar"), "⠁⠸");
        assert_eq!(pipeline.translate("  "), "⠀⠀");
        assert_eq!(pipeline.translate("🐂"), "⠳⠭⠂⠋⠲⠴⠆");
    }
}
