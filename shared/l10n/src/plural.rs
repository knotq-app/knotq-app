//! CLDR cardinal plural rules for the supported locales, specialized to
//! integer counts (the catalog only pluralizes whole-number counts).
//!
//! Reference: <https://www.unicode.org/cldr/charts/45/supplemental/language_plural_rules.html>
//! Adding a locale with rules not covered here = add a match arm; the default
//! `one`/`other` arm is correct for most European languages.

/// The CLDR cardinal category for `count` in `locale` (a supported code from
/// `locales.json`; only the language part is significant).
pub fn plural_category(locale: &str, count: i64) -> &'static str {
    let n = count.unsigned_abs();
    let language = locale.split('-').next().unwrap_or(locale);
    match language {
        // No plural distinctions.
        "ja" | "ko" | "zh" | "th" | "vi" | "id" | "ms" => "other",

        // `one` covers 0 and 1.
        "fr" | "hi" => {
            if n <= 1 {
                "one"
            } else {
                "other"
            }
        }

        // Brazilian Portuguese counts 0 as `one`; European Portuguese doesn't.
        "pt" => {
            if locale == "pt-PT" {
                if n == 1 {
                    "one"
                } else {
                    "other"
                }
            } else if n <= 1 {
                "one"
            } else {
                "other"
            }
        }

        "ru" | "uk" => {
            let (m10, m100) = (n % 10, n % 100);
            if m10 == 1 && m100 != 11 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }

        "pl" => {
            let (m10, m100) = (n % 10, n % 100);
            if n == 1 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }

        "cs" | "sk" => {
            if n == 1 {
                "one"
            } else if (2..=4).contains(&n) {
                "few"
            } else {
                "other"
            }
        }

        "ro" => {
            let m100 = n % 100;
            if n == 1 {
                "one"
            } else if n == 0 || (1..=19).contains(&m100) {
                "few"
            } else {
                "other"
            }
        }

        "ar" => match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            _ => match n % 100 {
                3..=10 => "few",
                11..=99 => "many",
                _ => "other",
            },
        },

        "he" => match n {
            1 => "one",
            2 => "two",
            _ => "other",
        },

        // English, German, Dutch, Scandinavian, Finnish, Greek, Hungarian,
        // Turkish, Italian, Spanish, European Portuguese, …
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plural_category as cat;

    #[test]
    fn english_and_default() {
        assert_eq!(cat("en", 1), "one");
        assert_eq!(cat("en", 0), "other");
        assert_eq!(cat("en", 5), "other");
        assert_eq!(cat("de", 1), "one");
        assert_eq!(cat("tr", 2), "other");
    }

    #[test]
    fn no_plural_languages() {
        for locale in ["ja", "ko", "zh-Hans", "zh-Hant", "th", "vi", "id", "ms"] {
            assert_eq!(cat(locale, 1), "other", "{locale}");
        }
    }

    #[test]
    fn french_hindi_zero_one() {
        assert_eq!(cat("fr", 0), "one");
        assert_eq!(cat("fr", 1), "one");
        assert_eq!(cat("fr", 2), "other");
        assert_eq!(cat("hi", 0), "one");
    }

    #[test]
    fn portuguese_variants() {
        assert_eq!(cat("pt-BR", 0), "one");
        assert_eq!(cat("pt-PT", 0), "other");
        assert_eq!(cat("pt-PT", 1), "one");
    }

    #[test]
    fn slavic() {
        assert_eq!(cat("ru", 1), "one");
        assert_eq!(cat("ru", 11), "many");
        assert_eq!(cat("ru", 21), "one");
        assert_eq!(cat("ru", 3), "few");
        assert_eq!(cat("ru", 13), "many");
        assert_eq!(cat("ru", 0), "many");
        assert_eq!(cat("pl", 1), "one");
        assert_eq!(cat("pl", 22), "few");
        assert_eq!(cat("pl", 12), "many");
        assert_eq!(cat("cs", 3), "few");
        assert_eq!(cat("cs", 5), "other");
    }

    #[test]
    fn romanian_arabic_hebrew() {
        assert_eq!(cat("ro", 1), "one");
        assert_eq!(cat("ro", 0), "few");
        assert_eq!(cat("ro", 119), "few");
        assert_eq!(cat("ro", 120), "other");
        assert_eq!(cat("ar", 0), "zero");
        assert_eq!(cat("ar", 2), "two");
        assert_eq!(cat("ar", 103), "few");
        assert_eq!(cat("ar", 111), "many");
        assert_eq!(cat("he", 2), "two");
        assert_eq!(cat("he", 20), "other");
    }
}
