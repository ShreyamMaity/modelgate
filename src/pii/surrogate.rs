const GIVEN: &[&str] = &[
    "Aarav",
    "Abhinav",
    "Aditi",
    "Advait",
    "Akhila",
    "Amrita",
    "Anika",
    "Anirudh",
    "Anushka",
    "Arjun",
    "Arnav",
    "Avantika",
    "Bhavya",
    "Chaitanya",
    "Charulata",
    "Darshan",
    "Devika",
    "Dhruv",
    "Divyansh",
    "Eshan",
    "Gargi",
    "Gautam",
    "Hansika",
    "Harsha",
    "Himani",
    "Ira",
    "Ishita",
    "Jahnavi",
    "Jatin",
    "Kabir",
    "Kalyani",
    "Kanishk",
    "Keerthana",
    "Kritika",
    "Lavanya",
    "Madhav",
    "Mahika",
    "Manasi",
    "Mihir",
    "Mridula",
    "Nakul",
    "Namrata",
    "Naveen",
    "Niharika",
    "Nikhita",
    "Ojas",
    "Paridhi",
    "Parth",
    "Pranjal",
    "Pratyush",
    "Radhika",
    "Raghav",
    "Ranjini",
    "Revathi",
    "Rishabh",
    "Riya",
    "Rudra",
    "Saanvi",
    "Samarth",
    "Sanchita",
    "Shaurya",
    "Shreya",
    "Siddhi",
    "Soham",
    "Srijan",
    "Tanishq",
    "Tanvi",
    "Tejas",
    "Trisha",
    "Udit",
    "Urvashi",
    "Vaibhav",
    "Vaidehi",
    "Vedant",
    "Vihaan",
    "Vrinda",
    "Yamini",
    "Yash",
    "Zoya",
    "Ayaan",
    "Aniket",
    "Bhargavi",
    "Debashree",
    "Farida",
    "Gunjan",
    "Hrithik",
    "Indrani",
    "Jayant",
    "Kaustubh",
    "Lalitha",
    "Madhuri",
    "Nirmal",
    "Omkar",
    "Pallabi",
    "Rituja",
    "Sameera",
    "Sharvari",
    "Sukanya",
    "Tarun",
    "Vasudha",
];

const FAMILY: &[&str] = &[
    "Acharya",
    "Ahluwalia",
    "Bajwa",
    "Banerjee",
    "Bhardwaj",
    "Bhatt",
    "Bose",
    "Chandran",
    "Chawla",
    "Dasgupta",
    "Deshpande",
    "Dhawan",
    "Dixit",
    "Gokhale",
    "Goswami",
    "Grewal",
    "Hegde",
    "Iyengar",
    "Jaiswal",
    "Kamath",
    "Kapadia",
    "Karnik",
    "Khanna",
    "Kulkarni",
    "Lahiri",
    "Mahajan",
    "Malhotra",
    "Mathur",
    "Menon",
    "Mitra",
    "Nadkarni",
    "Naidu",
    "Nambiar",
    "Oberoi",
    "Pai",
    "Panicker",
    "Parekh",
    "Patnaik",
    "Pillai",
    "Purohit",
    "Rajan",
    "Rangarajan",
    "Rathore",
    "Sabharwal",
    "Sahni",
    "Saraf",
    "Sarkar",
    "Sastry",
    "Sethi",
    "Shenoy",
    "Sinha",
    "Sodhi",
    "Subramaniam",
    "Talwar",
    "Thakkar",
    "Trivedi",
    "Upadhyay",
    "Vaidya",
    "Venkatesan",
    "Wadhwa",
    "Bhandari",
    "Chatterjee",
    "Dutta",
    "Ghosh",
    "Jhaveri",
    "Kashyap",
    "Lalwani",
    "Mukhopadhyay",
    "Narang",
    "Rao",
];

const ORG_HEAD: &[&str] = &[
    "Silverline",
    "Bluebay",
    "Sunpeak",
    "Harbourline",
    "Neelgiri",
    "Tamarind",
    "Crestview",
    "Riverstone",
    "Monsoonleaf",
    "Saffronfield",
    "Coralgate",
    "Indigo Crest",
    "Banyan Arc",
    "Teakwood",
    "Lotuspath",
    "Peacock Hill",
    "Cedarbrook",
    "Amberfort",
    "Stonebridge",
    "Jasminegate",
    "Copperleaf",
    "Mistvale",
    "Sandalwood",
    "Kingfisher Bay",
];

const ORG_TAIL: &[&str] = &[
    "Agro Pvt Ltd",
    "Logistics LLP",
    "Infotech Pvt Ltd",
    "Textiles",
    "Foods Pvt Ltd",
    "Analytics LLP",
    "Traders",
    "Ventures",
    "Systems Pvt Ltd",
    "Enterprises",
    "Consultants",
    "Exports",
];

const CITY: &[&str] = &[
    "Nashik",
    "Vadodara",
    "Mangaluru",
    "Mysuru",
    "Coimbatore",
    "Madurai",
    "Guntur",
    "Warangal",
    "Jabalpur",
    "Gwalior",
    "Udaipur",
    "Ajmer",
    "Dehradun",
    "Haridwar",
    "Kanpur",
    "Agra",
    "Meerut",
    "Ludhiana",
    "Jalandhar",
    "Amritsar",
    "Rajkot",
    "Surat",
    "Belagavi",
    "Hubballi",
    "Tiruchirappalli",
    "Salem",
    "Nellore",
    "Rourkela",
    "Cuttack",
    "Durgapur",
    "Asansol",
    "Jamshedpur",
    "Ranchi",
    "Raipur",
    "Bilaspur",
    "Kolhapur",
    "Solapur",
    "Aurangabad",
    "Thrissur",
    "Kozhikode",
];

const STREET: &[&str] = &[
    "Tagore",
    "Ashoka",
    "Sarojini",
    "Gokhale",
    "Tilak",
    "Rajaji",
    "Kamaraj",
    "Netaji",
    "Patel",
    "Shastri",
    "Vivekananda",
    "Ambedkar",
    "Azad",
    "Bhagat Singh",
    "Chitrakoot",
    "Ganga",
    "Yamuna",
    "Godavari",
    "Narmada",
    "Kaveri",
];

const STREET_KIND: &[&str] = &["Road", "Marg", "Street", "Lane", "Cross Road", "Main Road"];

const LOCALITY: &[&str] = &[
    "Shanti Nagar",
    "Rajendra Nagar",
    "Model Town",
    "Civil Lines",
    "Vasant Vihar",
    "Anand Colony",
    "Ram Nagar",
    "Green Park",
    "Prem Nagar",
    "Laxmi Colony",
    "Sundar Nagar",
    "Kalyan Nagar",
    "Tilak Nagar",
    "Govind Puri",
];

const BUILDING: &[&str] = &[
    "Sunrise Apartments",
    "Silver Oak Residency",
    "Gulmohar Heights",
    "Lakeview Enclave",
    "Shivam Towers",
    "Neelkanth Society",
    "Palm Grove Apartments",
    "Sai Darshan Complex",
];

pub fn is_surrogate_kind(kind: &str) -> bool {
    matches!(kind, "PERSON" | "NAME" | "ORG" | "LOCATION" | "ADDRESS")
}

pub fn mix(seed: u64, n: u64) -> u64 {
    let mut z = seed ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn pick<'a>(pool: &[&'a str], seed: u64, n: u64) -> &'a str {
    pool[(mix(seed, n) % pool.len() as u64) as usize]
}

pub fn given(seed: u64, n: u64) -> &'static str {
    pick(GIVEN, seed, n)
}

pub fn family(seed: u64, n: u64) -> &'static str {
    pick(FAMILY, seed, n)
}

pub fn pool_len(family_pool: bool) -> u64 {
    if family_pool {
        FAMILY.len() as u64
    } else {
        GIVEN.len() as u64
    }
}

pub fn whole(kind: &str, seed: u64, n: u64) -> String {
    match kind {
        "ORG" => format!(
            "{} {}",
            pick(ORG_HEAD, seed, n),
            pick(ORG_TAIL, seed, n + 7919)
        ),
        "LOCATION" => pick(CITY, seed, n).to_owned(),
        _ => {
            let house = 10 + mix(seed, n + 17) % 480;
            let pin = 110_000 + mix(seed, n + 31) % 740_000;
            format!(
                "{}, {}, {} {}, {}, {} {}",
                house,
                pick(BUILDING, seed, n + 3),
                pick(STREET, seed, n + 5),
                pick(STREET_KIND, seed, n + 11),
                pick(LOCALITY, seed, n + 13),
                pick(CITY, seed, n + 19),
                pin
            )
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Lower,
    Upper,
    Title,
    Mixed,
}

pub fn case_of(s: &str) -> Case {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return Case::Mixed;
    }
    if letters.iter().all(|c| c.is_lowercase()) {
        return Case::Lower;
    }
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        return Case::Upper;
    }
    let title = s.split_whitespace().all(|w| {
        let mut cs = w.chars().filter(|c| c.is_alphabetic());
        match cs.next() {
            Some(f) => f.is_uppercase() && cs.all(|c| c.is_lowercase()),
            None => true,
        }
    });
    if title {
        Case::Title
    } else {
        Case::Mixed
    }
}

fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            start = false;
        } else {
            out.push(c);
            if c.is_whitespace() || c == '-' {
                start = true;
            }
        }
    }
    out
}

pub fn apply_case(target: &str, like: &str) -> String {
    match case_of(like) {
        Case::Lower => target.to_lowercase(),
        Case::Upper => target.to_uppercase(),
        Case::Title if case_of(target) == Case::Lower => title(target),
        _ => target.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_are_clean() {
        for p in [GIVEN, FAMILY] {
            let mut seen = std::collections::HashSet::new();
            for w in p {
                assert!(w.chars().all(|c| c.is_ascii_alphabetic()), "{w}");
                assert!(seen.insert(w.to_ascii_lowercase()), "dup {w}");
            }
        }
        assert!(GIVEN
            .iter()
            .all(|g| !FAMILY.iter().any(|f| f.eq_ignore_ascii_case(g))));
    }

    #[test]
    fn whole_is_deterministic() {
        assert_eq!(whole("ORG", 7, 1), whole("ORG", 7, 1));
        assert!(whole("ADDRESS", 7, 1).contains(", "));
        assert!(CITY.contains(&whole("LOCATION", 3, 9).as_str()));
    }

    #[test]
    fn casing() {
        assert_eq!(apply_case("Kavya Rao", "priya iyer"), "kavya rao");
        assert_eq!(apply_case("Kavya Rao", "PRIYA IYER"), "KAVYA RAO");
        assert_eq!(apply_case("rahul", "Kavya"), "Rahul");
        assert_eq!(apply_case("McKenzie", "Kavya"), "McKenzie");
        assert_eq!(case_of("Priya"), Case::Title);
        assert_eq!(case_of("42"), Case::Mixed);
    }
}
