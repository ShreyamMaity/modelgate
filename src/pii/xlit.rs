use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

const TABLE: &[(&str, &str)] = &[
    ("aarav", "आरव"),
    ("abhinav", "अभिनव"),
    ("aditi", "अदिति"),
    ("advait", "अद्वैत"),
    ("akhila", "अखिला"),
    ("amrita", "अमृता"),
    ("anika", "अनिका"),
    ("anirudh", "अनिरुद्ध"),
    ("anushka", "अनुष्का"),
    ("arjun", "अर्जुन"),
    ("arnav", "अर्णव|अर्नव"),
    ("avantika", "अवंतिका"),
    ("bhavya", "भव्या|भव्य"),
    ("chaitanya", "चैतन्य|चैतन्या"),
    ("charulata", "चारुलता"),
    ("devika", "देविका"),
    ("dhruv", "ध्रुव"),
    ("divyansh", "दिव्यांश"),
    ("eshan", "ईशान|एशान"),
    ("gargi", "गार्गी"),
    ("gautam", "गौतम"),
    ("hansika", "हंसिका"),
    ("himani", "हिमानी"),
    ("ira", "इरा"),
    ("ishita", "इशिता"),
    ("jahnavi", "जाह्नवी|जान्हवी"),
    ("jatin", "जतिन"),
    ("kabir", "कबीर"),
    ("kalyani", "कल्याणी"),
    ("kanishk", "कनिष्क"),
    ("keerthana", "कीर्तना|कीर्थना"),
    ("kritika", "कृतिका"),
    ("lavanya", "लावण्या"),
    ("madhav", "माधव"),
    ("mahika", "माहिका|महिका"),
    ("manasi", "मानसी"),
    ("mihir", "मिहिर"),
    ("mridula", "मृदुला"),
    ("nakul", "नकुल"),
    ("niharika", "निहारिका"),
    ("nikhita", "निखिता"),
    ("parth", "पार्थ"),
    ("pranjal", "प्रांजल"),
    ("pratyush", "प्रत्यूष|प्रत्युष"),
    ("radhika", "राधिका"),
    ("raghav", "राघव"),
    ("ranjini", "रंजिनी"),
    ("revathi", "रेवती|रेवथी"),
    ("rishabh", "ऋषभ|रिषभ"),
    ("riya", "रिया"),
    ("rudra", "रुद्र"),
    ("saanvi", "सान्वी"),
    ("sanchita", "संचिता"),
    ("shreya", "श्रेया"),
    ("soham", "सोहम"),
    ("tanishq", "तनिष्क"),
    ("tanvi", "तन्वी"),
    ("tejas", "तेजस"),
    ("trisha", "तृषा|त्रिशा"),
    ("urvashi", "उर्वशी"),
    ("vaidehi", "वैदेही"),
    ("vihaan", "विहान"),
    ("vrinda", "वृंदा"),
    ("yamini", "यामिनी"),
    ("zoya", "ज़ोया"),
    ("ayaan", "अयान|आयान"),
    ("aniket", "अनिकेत"),
    ("bhargavi", "भार्गवी"),
    ("debashree", "देबश्री|देवश्री"),
    ("farida", "फ़रीदा"),
    ("hrithik", "ऋतिक|हृतिक"),
    ("indrani", "इंद्राणी"),
    ("jayant", "जयंत"),
    ("kaustubh", "कौस्तुभ"),
    ("lalitha", "ललिता|ललिथा"),
    ("madhuri", "माधुरी"),
    ("omkar", "ओंकार"),
    ("pallabi", "पल्लबी|पल्लवी"),
    ("rituja", "ऋतुजा|रितुजा"),
    ("sameera", "समीरा"),
    ("sharvari", "शर्वरी"),
    ("sukanya", "सुकन्या"),
    ("ahluwalia", "अहलूवालिया|अहलुवालिया"),
    ("bajwa", "बाजवा"),
    ("banerjee", "बनर्जी#ব্যানার্জি|ব্যানার্জী"),
    ("bhardwaj", "भारद्वाज"),
    ("bhatt", "भट्ट|भट"),
    ("bose", "बोस#বসু"),
    ("chandran", "चंद्रन"),
    ("chawla", "चावला"),
    ("dasgupta", "दासगुप्ता|दासगुप्त#দাশগুপ্ত"),
    ("deshpande", "देशपांडे"),
    ("dhawan", "धवन"),
    ("dixit", "दीक्षित"),
    ("gokhale", "गोखले"),
    ("goswami", "गोस्वामी"),
    ("grewal", "ग्रेवाल"),
    ("hegde", "हेगड़े|हेगडे"),
    ("iyengar", "अयंगर|आयंगर"),
    ("jaiswal", "जायसवाल|जैसवाल"),
    ("kamath", "कामत"),
    ("kapadia", "कपाड़िया"),
    ("karnik", "कर्णिक|कार्णिक"),
    ("khanna", "खन्ना"),
    ("kulkarni", "कुलकर्णी"),
    ("lahiri", "लाहिड़ी|लाहिरी"),
    ("malhotra", "मल्होत्रा"),
    ("mathur", "माथुर"),
    ("menon", "मेनन"),
    ("nadkarni", "नाडकर्णी|नादकर्णी"),
    ("naidu", "नायडू"),
    ("nambiar", "नांबियार|नम्बियार"),
    ("oberoi", "ओबेरॉय|ओबरॉय"),
    ("pai", "पई"),
    ("panicker", "पणिक्कर|पनिकर"),
    ("parekh", "पारेख"),
    ("patnaik", "पटनायक"),
    ("pillai", "पिल्लई|पिल्लै|पिलाई"),
    ("rajan", "राजन"),
    ("rangarajan", "रंगराजन"),
    ("rathore", "राठौर|राठौड़"),
    ("sabharwal", "सभरवाल"),
    ("sahni", "साहनी|सहनी"),
    ("saraf", "सराफ़|सर्राफ"),
    ("sastry", "शास्त्री|सास्त्री"),
    ("sethi", "सेठी"),
    ("shenoy", "शेनॉय|शेणई"),
    ("sinha", "सिन्हा"),
    ("sodhi", "सोढ़ी|सोधी"),
    ("subramaniam", "सुब्रमण्यम|सुब्रमणियम"),
    ("thakkar", "ठक्कर"),
    ("trivedi", "त्रिवेदी"),
    ("venkatesan", "वेंकटेसन"),
    ("wadhwa", "वाधवा"),
    ("bhandari", "भंडारी"),
    ("chatterjee", "चटर्जी#চ্যাটার্জি|চ্যাটার্জী"),
    ("dutta", "दत्ता|दत्त"),
    ("ghosh", "घोष"),
    ("jhaveri", "झवेरी"),
    ("kashyap", "कश्यप"),
    ("lalwani", "लालवानी"),
    ("mukhopadhyay", "मुखोपाध्याय"),
    ("narang", "नारंग"),
    ("rao", "राव"),
    ("nashik", "नाशिक"),
    ("vadodara", "वडोदरा"),
    ("mangaluru", "मंगलुरु|मंगलूरु"),
    ("mysuru", "मैसूरु|मैसूर"),
    ("coimbatore", "कोयंबटूर"),
    ("madurai", "मदुरै|मदुरई"),
    ("guntur", "गुंटूर"),
    ("warangal", "वारंगल"),
    ("jabalpur", "जबलपुर"),
    ("gwalior", "ग्वालियर"),
    ("udaipur", "उदयपुर"),
    ("ajmer", "अजमेर"),
    ("dehradun", "देहरादून"),
    ("haridwar", "हरिद्वार"),
    ("kanpur", "कानपुर"),
    ("agra", "आगरा"),
    ("meerut", "मेरठ"),
    ("ludhiana", "लुधियाना"),
    ("jalandhar", "जालंधर"),
    ("amritsar", "अमृतसर"),
    ("rajkot", "राजकोट"),
    ("belagavi", "बेलगावी|बेलगाम"),
    ("hubballi", "हुब्बल्ली|हुबली"),
    ("tiruchirappalli", "तिरुचिरापल्ली|त्रिची"),
    ("salem", "सेलम|सलेम"),
    ("nellore", "नेल्लोर"),
    ("rourkela", "राउरकेला"),
    ("cuttack", "कटक|कट्टक"),
    ("durgapur", "दुर्गापुर"),
    ("asansol", "आसनसोल"),
    ("jamshedpur", "जमशेदपुर"),
    ("ranchi", "रांची"),
    ("raipur", "रायपुर"),
    ("bilaspur", "बिलासपुर"),
    ("kolhapur", "कोल्हापुर"),
    ("solapur", "सोलापुर"),
    ("aurangabad", "औरंगाबाद"),
    ("thrissur", "त्रिशूर"),
    ("kozhikode", "कोझिकोड"),
    ("silverline", "सिल्वरलाइन|सिल्वर लाइन"),
    ("bluebay", "ब्लूबे|ब्लू बे"),
    ("sunpeak", "सनपीक|सन पीक"),
    ("harbourline", "हार्बरलाइन|हार्बर लाइन"),
    ("neelgiri", "नीलगिरी|नीलगिरि"),
    ("tamarind", "टैमरिंड|टेमरिंड|टैमारिंड|तामरिंद"),
    ("crestview", "क्रेस्टव्यू|क्रेस्ट व्यू"),
    ("riverstone", "रिवरस्टोन|रिवर स्टोन"),
    ("monsoonleaf", "मानसूनलीफ़|मानसून लीफ़|मॉनसून लीफ़"),
    ("saffronfield", "सैफ्रनफील्ड|सैफ्रन फील्ड"),
    ("coralgate", "कोरलगेट|कोरल गेट|कॉरल गेट"),
    ("indigo", "इंडिगो"),
    ("crest", "क्रेस्ट"),
    ("banyan", "बनियन|बन्यन|बनयान|बैनियन|बैन्यन|बेनियन"),
    ("arc", "आर्क"),
    ("teakwood", "टीकवुड|टीक वुड"),
    ("lotuspath", "लोटसपाथ|लोटस पाथ|लोटस पथ"),
    ("peacock", "पीकॉक|पीकोक"),
    ("hill", "हिल"),
    ("cedarbrook", "सीडरब्रुक|सीडर ब्रुक|सेडरब्रुक|सेडार ब्रुक"),
    ("amberfort", "एम्बरफोर्ट|एम्बर फोर्ट|अंबर फोर्ट"),
    ("stonebridge", "स्टोनब्रिज|स्टोन ब्रिज"),
    ("jasminegate", "जैस्मिनगेट|जैस्मिन गेट"),
    ("copperleaf", "कॉपरलीफ़|कॉपर लीफ़"),
    ("mistvale", "मिस्टवेल|मिस्ट वेल"),
    ("sandalwood", "सैंडलवुड|सैंडल वुड"),
    ("kingfisher", "किंगफिशर"),
    ("bay", "बे"),
    ("agro", "एग्रो"),
    ("pvt", "प्राइवेट|प्रा.|प्रा#প্রাইভেট|প্রা."),
    ("ltd", "लिमिटेड|लि.|लि#লিমিটেড|লি."),
    ("logistics", "लॉजिस्टिक्स"),
    ("llp", "एलएलपी"),
    ("infotech", "इन्फोटेक"),
    ("textiles", "टेक्सटाइल्स|टेक्सटाइल"),
    ("foods", "फूड्स"),
    ("analytics", "एनालिटिक्स|ऐनालिटिक्स"),
    ("traders", "ट्रेडर्स"),
    ("ventures", "वेंचर्स"),
    ("systems", "सिस्टम्स|सिस्टम"),
    ("enterprises", "एंटरप्राइजेज|एंटरप्राइज़ेज़|एंटरप्राइसेस"),
    ("consultants", "कंसल्टेंट्स"),
    ("exports", "एक्सपोर्ट्स"),
];

const CAP: usize = 56;
const FIRST_WORD: usize = 16;
const OTHER_WORD: usize = 3;
const PHRASE_CAP: usize = 32;

const VIRAMA: char = '\u{094D}';
const NUKTA: char = '\u{093C}';
const AA: char = '\u{093E}';
const ANUSVARA: char = '\u{0902}';
const B_NUKTA: char = '\u{09BC}';

const COMPOSED: &[(char, char)] = &[
    ('\u{0958}', '\u{0915}'),
    ('\u{0959}', '\u{0916}'),
    ('\u{095A}', '\u{0917}'),
    ('\u{095B}', '\u{091C}'),
    ('\u{095C}', '\u{0921}'),
    ('\u{095D}', '\u{0922}'),
    ('\u{095E}', '\u{092B}'),
    ('\u{095F}', '\u{092F}'),
    ('\u{09DC}', '\u{09A1}'),
    ('\u{09DD}', '\u{09A2}'),
    ('\u{09DF}', '\u{09AF}'),
];

const NUKTA_BASES: &[char] = &[
    '\u{0915}', '\u{0916}', '\u{0917}', '\u{091C}', '\u{0921}', '\u{0922}', '\u{092B}',
];

const SWAPS: &[(char, char)] = &[
    ('\u{093F}', '\u{0940}'),
    ('\u{0940}', '\u{093F}'),
    ('\u{0941}', '\u{0942}'),
    ('\u{0942}', '\u{0941}'),
    ('\u{0907}', '\u{0908}'),
    ('\u{0908}', '\u{0907}'),
    ('\u{0909}', '\u{090A}'),
    ('\u{090A}', '\u{0909}'),
    ('\u{0947}', '\u{0948}'),
    ('\u{0948}', '\u{0947}'),
    ('\u{094B}', '\u{094C}'),
    ('\u{094C}', '\u{094B}'),
    ('\u{0949}', '\u{094B}'),
    ('\u{094B}', '\u{0949}'),
    ('\u{0926}', '\u{0921}'),
    ('\u{0924}', '\u{091F}'),
    ('\u{0921}', '\u{0926}'),
    ('\u{091F}', '\u{0924}'),
    ('\u{0948}', '\u{093E}'),
    ('\u{0947}', '\u{093E}'),
    ('\u{0916}', '\u{0915}'),
    ('\u{0918}', '\u{0917}'),
    ('\u{091B}', '\u{091A}'),
    ('\u{091D}', '\u{091C}'),
    ('\u{0920}', '\u{091F}'),
    ('\u{0922}', '\u{0921}'),
    ('\u{0925}', '\u{0924}'),
    ('\u{0927}', '\u{0926}'),
    ('\u{092B}', '\u{092A}'),
    ('\u{092D}', '\u{092C}'),
    ('\u{0938}', '\u{0936}'),
    ('\u{0936}', '\u{0938}'),
    ('\u{0936}', '\u{0937}'),
    ('\u{0937}', '\u{0936}'),
    ('\u{0935}', '\u{092C}'),
    ('\u{092C}', '\u{0935}'),
    ('\u{0923}', '\u{0928}'),
    ('\u{0928}', '\u{0923}'),
    ('\u{0901}', '\u{0902}'),
    ('\u{0902}', '\u{0901}'),
];

const LENGTH_SWAPS: usize = 8;

const INITIAL_SWAPS: &[(char, char)] = &[
    ('\u{0905}', '\u{0906}'),
    ('\u{0906}', '\u{0905}'),
    ('\u{090F}', '\u{0910}'),
    ('\u{0910}', '\u{090F}'),
    ('\u{0913}', '\u{0914}'),
    ('\u{0914}', '\u{0913}'),
];

const STOP: &[&str] = &[
    "है",
    "और",
    "में",
    "का",
    "की",
    "के",
    "को",
    "से",
    "पर",
    "यह",
    "वह",
    "था",
    "थी",
    "थे",
    "नहीं",
    "क्या",
    "कर",
    "हो",
    "भी",
    "तो",
    "जो",
    "एक",
    "दो",
    "तीन",
    "लिए",
    "साथ",
    "बात",
    "काम",
    "नाम",
    "राम",
    "दिन",
    "रात",
    "आज",
    "कल",
    "अब",
    "सब",
    "बहुत",
    "अच्छा",
    "ठीक",
    "राजा",
    "रानी",
    "सरकार",
    "जान",
    "जाना",
    "माँ",
    "माता",
    "पिता",
    "भाई",
    "बहन",
    "घर",
    "पानी",
    "खाना",
    "समय",
    "साल",
    "देश",
    "शहर",
    "गांव",
    "लोग",
    "आप",
    "हम",
    "तुम",
    "मैं",
    "वो",
    "कौन",
    "कब",
    "कहाँ",
    "कैसे",
    "क्यों",
    "जब",
    "तब",
    "यहाँ",
    "वहाँ",
    "बड़ा",
    "छोटा",
    "नया",
    "पुराना",
    "सच",
    "झूठ",
    "प्यार",
    "दिल",
    "मन",
    "तन",
    "धन",
    "जन",
    "सुन",
    "देख",
    "बोल",
    "चल",
    "रख",
    "रहा",
    "रही",
    "रहे",
    "गया",
    "गई",
    "गए",
    "आया",
    "आई",
    "आए",
    "मिल",
    "मिला",
    "दे",
    "दिया",
    "ले",
    "लिया",
    "मत",
    "बस",
    "अरे",
    "हाँ",
    "ना",
    "जी",
    "श्री",
    "राज",
    "रवि",
    "सूरज",
    "चांद",
    "तारा",
    "धरती",
    "आकाश",
    "हवा",
    "आग",
    "नदी",
    "सागर",
    "पहाड़",
    "बाजार",
    "दुकान",
    "गाड़ी",
    "रेल",
    "सड़क",
    "फोन",
    "कंपनी",
    "दफ्तर",
    "ऑफिस",
    "टीम",
    "काजल",
    "कमल",
    "मोती",
    "हीरा",
    "सोना",
    "चांदी",
];

#[derive(Debug, Default)]
pub struct Forms {
    pub deva: Vec<String>,
    pub beng: Vec<String>,
}

fn index() -> &'static HashMap<&'static str, &'static str> {
    static I: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    I.get_or_init(|| TABLE.iter().copied().collect())
}

pub fn known(word: &str) -> bool {
    index().contains_key(word.to_ascii_lowercase().as_str())
}

pub fn word(w: &str) -> Option<Arc<Forms>> {
    static C: OnceLock<Mutex<HashMap<&'static str, Arc<Forms>>>> = OnceLock::new();
    let (k, v) = index().get_key_value(w.to_ascii_lowercase().as_str())?;
    let cache = C.get_or_init(Default::default);
    if let Some(f) = cache.lock().unwrap().get(k) {
        return Some(f.clone());
    }
    let f = Arc::new(build(v));
    cache.lock().unwrap().insert(k, f.clone());
    Some(f)
}

pub fn prime(sur: &str) {
    for w in sur.split_whitespace() {
        let _ = word(w);
    }
}

pub fn tail_tokens(sur: &str, head: usize) -> HashSet<String> {
    let mut out = HashSet::new();
    for w in sur.split_whitespace().skip(head.max(1)) {
        let l = w.to_ascii_lowercase();
        out.insert(format!("{l}."));
        out.insert(l);
        if let Some(f) = word(w) {
            out.extend(f.deva.iter().cloned());
            out.extend(f.beng.iter().cloned());
        }
    }
    out
}

pub fn is_stop(w: &str) -> bool {
    STOP.contains(&w)
}

fn class_of(c: char) -> Option<char> {
    let c = if ('\u{0980}'..='\u{09FF}').contains(&c) {
        char::from_u32(c as u32 - 0x80).unwrap_or(c)
    } else {
        c
    };
    let c = match COMPOSED.iter().find(|(p, _)| *p == c) {
        Some((_, b)) => *b,
        None => c,
    };
    Some(match c {
        '\u{0915}' | '\u{0916}' => 'K',
        '\u{0917}'..='\u{0918}' => 'G',
        '\u{0902}' | '\u{0919}' | '\u{091E}' | '\u{0923}' | '\u{0928}' => 'N',
        '\u{091A}' | '\u{091B}' => 'C',
        '\u{091C}' | '\u{091D}' => 'J',
        '\u{091F}'..='\u{0920}' | '\u{0924}'..='\u{0925}' => 'T',
        '\u{0921}'..='\u{0922}' | '\u{0926}'..='\u{0927}' => 'D',
        '\u{092A}' | '\u{092B}' => 'P',
        '\u{092C}' | '\u{092D}' | '\u{0935}' => 'B',
        '\u{092E}' => 'M',
        '\u{092F}' => 'Y',
        '\u{0930}' | '\u{090B}' | '\u{0943}' => 'R',
        '\u{0932}' | '\u{0933}' => 'L',
        '\u{0936}'..='\u{0938}' => 'S',
        '\u{0939}' => 'H',
        '\u{0905}'..='\u{0914}' => 'A',
        _ => return None,
    })
}

pub fn skeleton(word: &str) -> Option<String> {
    let mut out = String::new();
    let mut last = None;
    let mut consonants = 0;
    for (i, c) in word.chars().enumerate() {
        let Some(k) = class_of(c) else {
            if is_indic(c) || is_mark(c) {
                continue;
            }
            return None;
        };
        if k == 'A' {
            if i == 0 {
                out.push('A');
            }
            continue;
        }
        if k == 'Y' && i > 0 {
            continue;
        }
        if last != Some(k) {
            out.push(k);
            consonants += 1;
        }
        last = Some(k);
    }
    (consonants >= 3).then_some(out)
}

pub fn is_indic(c: char) -> bool {
    ('\u{0900}'..='\u{09FF}').contains(&c) && !matches!(c, '\u{0964}' | '\u{0965}')
}

pub fn is_mark(c: char) -> bool {
    matches!(c,
        '\u{0900}'..='\u{0903}'
        | '\u{093A}'..='\u{094F}'
        | '\u{0951}'..='\u{0957}'
        | '\u{0962}'..='\u{0963}'
        | '\u{0981}'..='\u{0983}'
        | '\u{09BC}'
        | '\u{09BE}'..='\u{09CD}'
        | '\u{09D7}'
        | '\u{09E2}'..='\u{09E3}'
        | '\u{200C}'
        | '\u{200D}')
}

pub fn has_indic(s: &str) -> bool {
    s.as_bytes()
        .windows(2)
        .any(|w| w[0] == 0xE0 && (0xA4..=0xA7).contains(&w[1]))
}

pub fn is_bengali(s: &str) -> bool {
    s.chars().any(|c| ('\u{0980}'..='\u{09FF}').contains(&c))
}

pub fn indic_runs(text: &str) -> String {
    let mut out = String::new();
    let mut cur = String::new();
    let mut indic = false;
    for c in text.chars() {
        if is_indic(c) || is_mark(c) || c == ' ' || c == '.' {
            cur.push(c);
            indic |= is_indic(c);
        } else {
            if indic {
                out.push_str(cur.trim());
                out.push('\n');
            }
            cur.clear();
            indic = false;
        }
    }
    if indic {
        out.push_str(cur.trim());
    }
    out
}

fn deva_cons(c: char) -> bool {
    ('\u{0915}'..='\u{0939}').contains(&c) || ('\u{0958}'..='\u{095F}').contains(&c)
}

fn cons_end(v: &[char], i: usize) -> bool {
    deva_cons(v[i]) || (v[i] == NUKTA && i > 0 && deva_cons(v[i - 1]))
}

fn decompose(s: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match COMPOSED.iter().find(|(p, _)| *p == c) {
            Some((_, base)) => {
                out.push(*base);
                out.push(if ('\u{0980}'..='\u{09FF}').contains(base) {
                    B_NUKTA
                } else {
                    NUKTA
                });
            }
            None => out.push(c),
        }
    }
    out
}

fn compose(v: &[char]) -> Option<Vec<char>> {
    let mut out = Vec::with_capacity(v.len());
    let mut changed = false;
    let mut i = 0;
    while i < v.len() {
        if i + 1 < v.len() && (v[i + 1] == NUKTA || v[i + 1] == B_NUKTA) {
            if let Some((p, _)) = COMPOSED.iter().find(|(_, b)| *b == v[i]) {
                out.push(*p);
                i += 2;
                changed = true;
                continue;
            }
        }
        out.push(v[i]);
        i += 1;
    }
    changed.then_some(out)
}

fn nasal_for(c: char) -> Option<char> {
    match c {
        '\u{0915}'..='\u{0919}' => Some('\u{0919}'),
        '\u{091A}'..='\u{091E}' => Some('\u{091E}'),
        '\u{091F}'..='\u{0923}' => Some('\u{0923}'),
        '\u{0924}'..='\u{0928}' => Some('\u{0928}'),
        '\u{092A}'..='\u{092E}' => Some('\u{092E}'),
        _ => None,
    }
}

fn splice(v: &[char], at: usize, len: usize, with: &[char]) -> Vec<char> {
    let mut n = Vec::with_capacity(v.len() + with.len());
    n.extend_from_slice(&v[..at]);
    n.extend_from_slice(with);
    n.extend_from_slice(&v[at + len..]);
    n
}

fn neighbours(v: &[char]) -> Vec<Vec<char>> {
    let mut hi = Vec::new();
    let mut lo = Vec::new();
    let n = v.len();
    for i in 0..n {
        let c = v[i];
        let start = i == 0 || v[i - 1] == ' ';
        let end = i + 1 == n || v[i + 1] == ' ';
        for (k, (a, b)) in SWAPS.iter().enumerate() {
            if c == *a {
                let x = splice(v, i, 1, &[*b]);
                if k < LENGTH_SWAPS {
                    hi.push(x);
                } else {
                    lo.push(x);
                }
            }
        }
        if start {
            for (a, b) in INITIAL_SWAPS {
                if c == *a {
                    lo.push(splice(v, i, 1, &[*b]));
                }
            }
            if c == '\u{090B}' {
                lo.push(splice(v, i, 1, &['\u{0930}', '\u{093F}']));
            }
            if c == '\u{0930}' && v.get(i + 1) == Some(&'\u{093F}') {
                lo.push(splice(v, i, 2, &['\u{090B}']));
            }
        }
        if c == '\u{0943}' {
            lo.push(splice(v, i, 1, &[VIRAMA, '\u{0930}', '\u{093F}']));
        }
        if c == VIRAMA && v.get(i + 1) == Some(&'\u{0930}') && v.get(i + 2) == Some(&'\u{093F}') {
            lo.push(splice(v, i, 3, &['\u{0943}']));
        }
        if NUKTA_BASES.contains(&c) && v.get(i + 1) != Some(&NUKTA) {
            hi.push(splice(v, i + 1, 0, &[NUKTA]));
        }
        if c == NUKTA {
            hi.push(splice(v, i, 1, &[]));
        }
        if c == VIRAMA && i > 0 && cons_end(v, i - 1) && v.get(i + 1).is_some_and(|x| deva_cons(*x))
        {
            hi.push(splice(v, i, 1, &[]));
            if deva_cons(v[i - 1]) && v.get(i + 1) == Some(&v[i - 1]) {
                hi.push(splice(v, i, 2, &[]));
            }
        }
        if c == ANUSVARA {
            if let Some(m) = v.get(i + 1).and_then(|x| nasal_for(*x)) {
                hi.push(splice(v, i, 1, &[m, VIRAMA]));
            }
        }
        if matches!(
            c,
            '\u{0919}' | '\u{091E}' | '\u{0923}' | '\u{0928}' | '\u{092E}'
        ) && v.get(i + 1) == Some(&VIRAMA)
            && v.get(i + 2).is_some_and(|x| deva_cons(*x))
        {
            hi.push(splice(v, i, 2, &[ANUSVARA]));
        }
        if !end && c == AA && i > 0 && cons_end(v, i - 1) {
            hi.push(splice(v, i, 1, &[]));
        }
        if !end && cons_end(v, i) && v.get(i + 1).is_some_and(|x| deva_cons(*x)) {
            lo.push(splice(v, i + 1, 0, &[AA]));
        }
        if end {
            let tok = v[..=i].iter().rev().take_while(|x| **x != ' ').count();
            if cons_end(v, i) {
                hi.push(splice(v, i + 1, 0, &[AA]));
            }
            if c == AA && i > 0 && cons_end(v, i - 1) && tok > 2 {
                hi.push(splice(v, i, 1, &[]));
            }
        }
    }
    hi.extend(lo);
    hi
}

fn expand(seeds: Vec<Vec<char>>, step: fn(&[char]) -> Vec<Vec<char>>) -> Vec<Vec<char>> {
    let mut seen: HashSet<Vec<char>> = HashSet::new();
    let mut out: Vec<Vec<char>> = Vec::new();
    let mut frontier = Vec::new();
    for s in seeds {
        if !s.is_empty() && seen.insert(s.clone()) {
            out.push(s.clone());
            frontier.push(s);
        }
    }
    for _ in 0..2 {
        let mut next = Vec::new();
        for f in &frontier {
            for n in step(f) {
                if out.len() >= CAP {
                    return out;
                }
                if seen.insert(n.clone()) {
                    out.push(n.clone());
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    out
}

fn to_beng(v: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(v.len() + 2);
    for (i, &c) in v.iter().enumerate() {
        let prev = if i == 0 { None } else { Some(v[i - 1]) };
        let start = prev.is_none() || prev == Some(' ');
        match c {
            '\u{0935}' => out.push('\u{09AC}'),
            '\u{092F}' => {
                out.push('\u{09AF}');
                if !(start || prev == Some(VIRAMA)) {
                    out.push(B_NUKTA);
                }
            }
            '\u{0949}' => out.push('\u{09CB}'),
            '\u{0945}' => out.push('\u{09C7}'),
            '\u{0911}' => out.push('\u{0985}'),
            '\u{090D}' => out.push('\u{098F}'),
            NUKTA => {
                if matches!(prev, Some('\u{0921}' | '\u{0922}')) {
                    out.push(B_NUKTA);
                }
            }
            '\u{0900}'..='\u{097F}' => {
                out.push(char::from_u32(c as u32 + 0x80).unwrap_or(c));
            }
            _ => out.push(c),
        }
    }
    out
}

fn beng_neighbours(v: &[char]) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    for i in 0..v.len() {
        if v[i] == '\u{09AF}' && i > 0 && v[i - 1] != ' ' && v[i - 1] != '\u{09CD}' {
            if v.get(i + 1) == Some(&B_NUKTA) {
                out.push(splice(v, i + 1, 1, &[]));
            } else {
                out.push(splice(v, i + 1, 0, &[B_NUKTA]));
            }
        }
        if v[i] == '\u{09B8}' {
            out.push(splice(v, i, 1, &['\u{09B6}']));
        }
        if v[i] == '\u{09B6}' {
            out.push(splice(v, i, 1, &['\u{09B8}']));
        }
    }
    out
}

fn with_encodings(forms: Vec<Vec<char>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(forms.len() * 2);
    for f in forms {
        let alt = compose(&f);
        for v in std::iter::once(f).chain(alt) {
            let s: String = v.into_iter().collect();
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
    }
    out
}

fn build(v: &str) -> Forms {
    let (d, b) = v.split_once('#').unwrap_or((v, ""));
    let deva = expand(d.split('|').map(decompose).collect(), neighbours);
    let mut bseeds: Vec<Vec<char>> = b
        .split('|')
        .filter(|s| !s.is_empty())
        .map(decompose)
        .collect();
    bseeds.extend(deva.iter().map(|f| to_beng(f)));
    let mut bseen = HashSet::new();
    let mut beng: Vec<Vec<char>> = Vec::new();
    for s in bseeds {
        if beng.len() >= CAP {
            break;
        }
        if bseen.insert(s.clone()) {
            beng.push(s);
        }
    }
    let heads: Vec<Vec<char>> = beng.iter().take(4).cloned().collect();
    for h in heads {
        for n in beng_neighbours(&h) {
            if beng.len() >= CAP + 8 {
                break;
            }
            if bseen.insert(n.clone()) {
                beng.push(n);
            }
        }
    }
    let min = d
        .split('|')
        .map(|x| x.chars().count())
        .min()
        .unwrap_or(3)
        .min(3);
    let keep = |v: Vec<String>| -> Vec<String> {
        v.into_iter()
            .filter(|f| f.chars().count() >= min && !STOP.contains(&f.as_str()))
            .collect()
    };
    Forms {
        deva: keep(with_encodings(deva)),
        beng: keep(with_encodings(beng)),
    }
}

fn script_forms(f: &Forms, beng: bool) -> &[String] {
    if beng {
        &f.beng
    } else {
        &f.deva
    }
}

fn combos(words: &[&str], beng: bool) -> Vec<String> {
    let forms: Vec<Option<Arc<Forms>>> = words.iter().map(|w| word(w)).collect();
    if forms.iter().all(Option::is_none) {
        return Vec::new();
    }
    let opts: Vec<Vec<&str>> = words
        .iter()
        .zip(&forms)
        .enumerate()
        .map(|(i, (w, f))| {
            let lim = if i == 0 { FIRST_WORD } else { OTHER_WORD };
            let mut o: Vec<&str> = f
                .as_ref()
                .map(|f| {
                    script_forms(f, beng)
                        .iter()
                        .take(lim)
                        .map(String::as_str)
                        .collect()
                })
                .unwrap_or_default();
            o.push(w);
            o
        })
        .collect();
    let latin: Vec<&str> = words.to_vec();
    let base: Vec<&str> = opts.iter().map(|o| o[0]).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |v: &[&str]| {
        if v != latin.as_slice() && out.len() < PHRASE_CAP {
            let j = v.join(" ");
            if seen.insert(j.clone()) {
                out.push(j);
            }
        }
    };
    for f in &opts[0] {
        let mut v = base.clone();
        v[0] = f;
        push(&v);
        let mut l = latin.clone();
        l[0] = f;
        push(&l);
    }
    for k in 1..opts.len() {
        for o in &opts[k] {
            let mut v = base.clone();
            v[k] = o;
            push(&v);
        }
    }
    out
}

type PhraseCache = Mutex<HashMap<(String, usize), Arc<[String]>>>;

fn phrase_cache() -> &'static PhraseCache {
    static C: OnceLock<PhraseCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub fn phrase_forms(sur: &str, prefix_from: Option<usize>) -> Arc<[String]> {
    let key = (sur.to_owned(), prefix_from.unwrap_or(0));
    if let Some(v) = phrase_cache().lock().unwrap().get(&key) {
        return v.clone();
    }
    let words: Vec<&str> = sur.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    if words.len() == 1 {
        if let Some(f) = word(words[0]) {
            out.extend(f.deva.iter().cloned());
            out.extend(f.beng.iter().cloned());
        }
    } else {
        for beng in [false, true] {
            out.extend(combos(&words, beng));
        }
        if let Some(k) = prefix_from.filter(|k| *k < words.len()) {
            let head = &words[..k.max(1)];
            if head.len() == 1 {
                if let Some(f) = word(head[0]) {
                    out.extend(f.deva.iter().cloned());
                    out.extend(f.beng.iter().cloned());
                }
            } else {
                for beng in [false, true] {
                    out.extend(combos(head, beng));
                }
            }
        }
    }
    let v: Arc<[String]> = out.into();
    let mut c = phrase_cache().lock().unwrap();
    if c.len() >= 1024 {
        c.clear();
    }
    c.insert(key, v.clone());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beng_assigned(c: char) -> bool {
        matches!(c,
            '\u{0980}'..='\u{0983}'
            | '\u{0985}'..='\u{098C}'
            | '\u{098F}'..='\u{0990}'
            | '\u{0993}'..='\u{09A8}'
            | '\u{09AA}'..='\u{09B0}'
            | '\u{09B2}'
            | '\u{09B6}'..='\u{09B9}'
            | '\u{09BC}'..='\u{09C4}'
            | '\u{09C7}'..='\u{09C8}'
            | '\u{09CB}'..='\u{09CE}'
            | '\u{09D7}'
            | '\u{09DC}'..='\u{09DD}'
            | '\u{09DF}'..='\u{09E3}')
    }

    #[test]
    fn every_pool_word_has_a_spelling() {
        let missing: Vec<&str> = super::super::surrogate::pool_words()
            .filter(|w| !known(w))
            .collect();
        assert!(missing.is_empty(), "no transliteration for {missing:?}");
    }

    #[test]
    fn table_is_clean() {
        let mut keys = HashSet::new();
        for (k, v) in TABLE {
            assert!(keys.insert(*k), "dup {k}");
            assert!(k.chars().all(|c| c.is_ascii_lowercase()), "{k}");
            let (d, b) = v.split_once('#').unwrap_or((v, ""));
            for c in d.chars() {
                assert!(
                    ('\u{0900}'..='\u{097F}').contains(&c) || matches!(c, ' ' | '.' | '|'),
                    "{k}: {c:?}"
                );
            }
            for c in b.chars() {
                assert!(beng_assigned(c) || c == '|' || c == ' ' || c == '.', "{k}");
            }
        }
    }

    #[test]
    fn forms_are_valid_script_and_bounded() {
        for (k, _) in TABLE {
            let f = word(k).unwrap();
            assert!(!f.deva.is_empty() && !f.beng.is_empty(), "{k}");
            assert!(
                f.deva.len() <= CAP * 2 && f.beng.len() <= (CAP + 8) * 2,
                "{k}"
            );
            for s in &f.deva {
                assert!(
                    s.chars()
                        .all(|c| ('\u{0900}'..='\u{097F}').contains(&c) || c == ' ' || c == '.'),
                    "{k}: {s}"
                );
            }
            for s in &f.beng {
                assert!(
                    s.chars().all(|c| beng_assigned(c) || c == ' ' || c == '.'),
                    "{k}: {s} {:?}",
                    s.chars().map(|c| c as u32).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn forms_avoid_common_words() {
        let common: HashSet<&str> = "है और में का की के को से पर यह वह था थी थे नहीं क्या कर हो भी तो जो एक दो तीन लिए साथ बात काम नाम राम दिन रात आज कल अब सब बहुत अच्छा ठीक राजा रानी सरकार जान जाना माँ माता पिता भाई बहन घर पानी खाना समय साल देश शहर गांव लोग आप हम तुम मैं वो कौन कब कहाँ कैसे क्यों जब तब यहाँ वहाँ बड़ा छोटा नया पुराना सच झूठ प्यार दिल मन तन धन जन सुन देख बोल चल रख रहा रही रहे गया गई गए आया आई आए मिल मिला दे दिया ले लिया मत बस अरे हाँ ना जी श्री राज रवि सूरज चांद तारा धरती आकाश हवा आग नदी सागर पहाड़ बाजार दुकान गाड़ी रेल सड़क फोन कंपनी दफ्तर ऑफिस टीम काजल कमल मोती हीरा सोना चांदी".split(' ').collect();
        let mut hits = Vec::new();
        for (k, _) in TABLE {
            let f = word(k).unwrap();
            for s in f.deva.iter() {
                if common.contains(s.as_str()) {
                    hits.push(format!("{k}:{s}"));
                }
            }
        }
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn forms_are_deterministic() {
        let a = build(index()["dasgupta"]);
        let b = build(index()["dasgupta"]);
        assert_eq!(a.deva, b.deva);
        assert_eq!(a.beng, b.beng);
    }

    fn has(w: &str, form: &str) -> bool {
        let f = word(w).unwrap();
        f.deva.iter().chain(&f.beng).any(|s| s == form)
    }

    #[test]
    fn common_variants() {
        assert!(has("Ayaan", "अयान"));
        assert!(has("Ayaan", "आयान"));
        assert!(has("Dasgupta", "दासगुप्त"));
        assert!(has("Dasgupta", "दासगुप्ता"));
        assert!(has("Dasgupta", "दाशगुप्त"));
        assert!(has("Dasgupta", "দাশগুপ্ত"));
        assert!(has("Dasgupta", "দাসগুপ্ত"));
        assert!(has("Malhotra", "मलहोत्रा"));
        assert!(has("Avantika", "अवन्तिका"));
        assert!(has("Riya", "रीया"));
        assert!(has("Riya", "রিয়া"));
        assert!(has("Riya", "রিয\u{09BC}া"));
        assert!(has("Zoya", "ज़ोया"));
        assert!(has("Zoya", "ज\u{093C}ोया"));
        assert!(has("Zoya", "जोया"));
        assert!(has("Farida", "फरीदा"));
        assert!(has("Hegde", "हेगडे"));
        assert!(has("Hegde", "हेग\u{095C}े"));
        assert!(has("Kritika", "क्रितिका"));
        assert!(has("Bhandari", "भण्डारी"));
        assert!(has("Banerjee", "ব্যানার্জি"));
        assert!(has("Mukhopadhyay", "মুখোপাধ্যায়"));
        assert!(has("Pallabi", "পল্লবী"));
        assert!(has("Rajan", "रजन"));
        assert!(has("Menon", "मेनान"));
        assert!(has("Banyan", "बैन्यन"));
        assert!(has("Debashree", "डेबाश्री"));
        assert!(has("Pillai", "पिलाई"));
        assert!(has("Cuttack", "कट्टक"));
        assert!(has("Khanna", "खना"));
        assert!(has("Hegde", "हेगदे"));
        assert!(has("Sabharwal", "सबरवाल"));
        assert!(has("Cedarbrook", "सेडरब्रूक"));
    }

    #[test]
    fn phrases_cover_names_and_orgs() {
        let p = phrase_forms("Ayaan Dasgupta", None);
        assert!(p.iter().any(|s| s == "अयान दासगुप्त"));
        assert!(p.iter().any(|s| s == "অয়ান দাশগুপ্ত"));
        assert!(p.len() <= PHRASE_CAP * 2);
        let o = phrase_forms("Amberfort Agro Pvt Ltd", Some(1));
        assert!(o.len() <= PHRASE_CAP * 2 + (CAP + 8) * 4);
        assert!(o.iter().any(|s| s == "एम्बरफोर्ट एग्रो प्राइवेट लिमिटेड"));
        assert!(o.iter().any(|s| s == "एम्बरफोर्ट Agro Pvt Ltd"));
        assert!(o.iter().any(|s| s == "एम्बरफोर्ट"));
        assert!(!o.iter().any(|s| s == "Amberfort Agro Pvt Ltd"));
        assert!(phrase_forms("Qwxyz", None).is_empty());
    }

    #[test]
    fn skeletons() {
        assert_eq!(skeleton("ठाकुर"), skeleton("ठक्कर"));
        assert_eq!(skeleton("पैनिकर"), skeleton("पणिक्कर"));
        assert_eq!(skeleton("भंडारी"), skeleton("भन्डारि"));
        assert_eq!(skeleton("দাশগুপ্ত"), skeleton("दासगुप्ता"));
        assert_eq!(skeleton("पटनाइक"), skeleton("पटनायक"));
        assert_eq!(skeleton("अयान"), None);
        assert_eq!(skeleton("राव"), None);
        assert_eq!(skeleton("Riya"), None);
    }

    #[test]
    fn helpers() {
        assert!(has_indic("abc अ"));
        assert!(has_indic("রিয়া"));
        assert!(!has_indic("plain ascii é"));
        assert!(is_mark('\u{094D}') && is_mark('\u{093C}') && !is_mark('क'));
        assert_eq!(indic_runs("hi रिया जी, ok राव."), "रिया जी\nराव.");
    }
}
