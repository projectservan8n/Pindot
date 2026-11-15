//! Extra Phonon voices (Apache-2.0) from huggingface.co/gradium/phonon-voices,
//! pinned to one commit with each file's size and SHA-256, like the models.
//! English only: the Phonon model in the catalog speaks English. A voice
//! (256 KB) is downloaded when the user picks or previews it in Settings,
//! into `<models dir>/phonon-voices/`, and checked on every load. The voice
//! loop never downloads one; a missing voice falls back to the default.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::Serialize;

use super::models::{self, APACHE_2_0, ModelFile, ModelKind, ModelSpec};

/// Folder under the models dir that holds the downloaded voices.
pub const VOICES_DIR: &str = "phonon-voices";
pub const SOURCE: &str = "huggingface.co/gradium/phonon-voices@da6c9ec";
const BASE_URL: &str =
    "https://huggingface.co/gradium/phonon-voices/resolve/da6c9ec0d47be8df5c87c730a3aa97187f3229c3/voices/";

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PhononVoice {
    pub name: &'static str,
    /// `en-us`, `en-gb`, or `characters` (character voices, no gender or age).
    pub locale: &'static str,
    pub gender: &'static str,
    pub age: &'static str,
    pub description: &'static str,
    #[serde(skip)]
    pub file: ModelFile,
}

/// `None`, empty or "default": the model's own voice.
pub fn is_default(setting: Option<&str>) -> bool {
    setting.is_none_or(|s| s.trim().is_empty() || s.eq_ignore_ascii_case("default"))
}

pub fn find(name: &str) -> Option<&'static PhononVoice> {
    VOICES.iter().find(|v| v.name.eq_ignore_ascii_case(name.trim()))
}

pub fn setting_ok(setting: Option<&str>) -> bool {
    is_default(setting) || setting.and_then(find).is_some()
}

/// The voice as a one-file model, so the model downloader fetches, resumes
/// and verifies it.
fn spec(v: &'static PhononVoice) -> ModelSpec {
    ModelSpec {
        id: VOICES_DIR,
        kind: ModelKind::TextToSpeech,
        display: v.name,
        license: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_text: APACHE_2_0,
        source: SOURCE,
        base_url: BASE_URL,
        files: std::slice::from_ref(&v.file),
        fallback: None,
    }
}

fn lookup(setting: Option<&str>) -> Result<Option<&'static PhononVoice>> {
    if is_default(setting) {
        return Ok(None);
    }
    let name = setting.unwrap_or_default();
    find(name).map(Some).ok_or_else(|| anyhow!("unknown voice \"{name}\""))
}

/// The chosen voice's name and verified file; `None` for the default voice.
/// Never downloads.
pub fn present(setting: Option<&str>, root: &Path) -> Result<Option<(&'static str, PathBuf)>> {
    let Some(v) = lookup(setting)? else { return Ok(None) };
    let dir = models::verify(&spec(v), root)?;
    Ok(Some((v.name, dir.join(v.file.name))))
}

/// As `present`, downloading (and verifying) the voice first if needed.
pub fn ensure(setting: Option<&str>, root: &Path) -> Result<Option<(&'static str, PathBuf)>> {
    let Some(v) = lookup(setting)? else { return Ok(None) };
    let dir = models::ensure(&spec(v), root, &mut |_| {})?;
    Ok(Some((v.name, dir.join(v.file.name))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_pinned_and_english() {
        assert_eq!(VOICES.len(), 37);
        let mut names: Vec<_> = VOICES.iter().map(|v| v.name.to_lowercase()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), VOICES.len(), "voice names must be unique");
        for v in VOICES {
            assert!(["en-us", "en-gb", "characters"].contains(&v.locale), "{}", v.name);
            assert_eq!(v.file.size, 256_136, "{}", v.name);
            assert_eq!(v.file.sha256.len(), 64, "{}", v.name);
            assert!(v.file.name.ends_with(".safetensors") && !v.file.name.contains('/'));
        }
    }

    #[test]
    fn settings_name_a_catalog_voice_or_the_default() {
        assert!(setting_ok(None) && setting_ok(Some("default")) && setting_ok(Some("")));
        assert!(setting_ok(Some("Zoey")) && setting_ok(Some("zoey")));
        assert!(!setting_ok(Some("Nobody")));
        assert_eq!(find("ZOEY").map(|v| v.name), Some("Zoey"));
    }

    #[test]
    fn a_missing_voice_is_an_error_and_the_default_needs_no_file() {
        let root = std::env::temp_dir().join(format!("deskhand-voices-{}", uuid::Uuid::new_v4()));
        assert!(present(None, &root).unwrap().is_none());
        assert!(present(Some("Zoey"), &root).is_err());
        assert!(present(Some("Nobody"), &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore = "downloads 256 KB from Hugging Face and needs the Phonon model in the models dir"]
    fn a_voice_downloads_verifies_and_speaks() {
        use crate::voice::engines::{PhononTts, TextToSpeech};
        let root = std::env::temp_dir().join(format!("deskhand-voices-{}", uuid::Uuid::new_v4()));
        let (name, path) = ensure(Some("zoey"), &root).unwrap().unwrap();
        assert_eq!(name, "Zoey");
        assert_eq!(present(Some("Zoey"), &root).unwrap().unwrap().1, path);
        let tts_dir =
            models::resolve(models::DEFAULT_TTS_MODEL, ModelKind::TextToSpeech, &models::models_dir().unwrap()).unwrap();
        let tts = PhononTts::load(&tts_dir, Some((name, &path))).unwrap();
        let mut samples = 0;
        tts.speak("Hello there.", &mut |c| {
            samples += c.len();
            true
        })
        .unwrap();
        assert!(samples > tts.sample_rate() as usize / 4, "only {samples} samples");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// English voices of the pinned catalog, by name. Generated from its
/// `voices.jsonl` and the Hugging Face file list (LFS SHA-256).
pub const VOICES: &[PhononVoice] = &[
    PhononVoice {
        name: "Alfie",
        locale: "en-gb",
        gender: "male",
        age: "Young Adult",
        description: "Punchy and forward, with crisp consonant attack and a pitch that lifts across a line, sitting high over a music bed.",
        file: ModelFile { name: "X6NceqF285Gr04gb.safetensors", size: 256136, sha256: "4de75497fff3030168e8aa6dca23ffeb332b1bc8c25d0bf3507ceadac953ae9c" },
    },
    PhononVoice {
        name: "Azgorath",
        locale: "characters",
        gender: "",
        age: "",
        description: "An enormous ancient demon. A cavernous, impossibly deep bass voice that rumbles like stone grinding in a crypt, with a dry rasping edge and a faint hiss on every s. Calm, slow and amused, never shouting, every syllable set down deliberately, menace held under perfect control.",
        file: ModelFile { name: "dGyGDKo2osj7h3s7.safetensors", size: 256136, sha256: "83900e020c14946426614c6553c5a138ba6277e1aaa69832223bd0f0f8b73893" },
    },
    PhononVoice {
        name: "Bryony",
        locale: "en-gb",
        gender: "female",
        age: "Middle-age",
        description: "Full, rich mid-range tones, softly relaxed and spoken close to the mic, moving briskly with light pitch play.",
        file: ModelFile { name: "gfP44RlHwr2kEwk7.safetensors", size: 256136, sha256: "bcc207a73b7885e45b49ab633d3377a10158292e3a7cef9cd6764ad107f1cb9b" },
    },
    PhononVoice {
        name: "Captain Silas Blackwater",
        locale: "characters",
        gender: "",
        age: "",
        description: "An old British pirate captain from the West Country, around sixty. Deep, gravelly and booming, rolling his Rs, salt-cracked from decades of shouting over gales. Roguish and hearty, with a wicked chuckle under every boast.",
        file: ModelFile { name: "xPDpsV3F592srkjA.safetensors", size: 256136, sha256: "a09f615b0084980ebc4076d71550bf73ccd8d4df076bbbee4a9beb523a1379d9" },
    },
    PhononVoice {
        name: "Delia",
        locale: "en-us",
        gender: "female",
        age: "Middle-age",
        description: "Low, grounded and quick to the point, with a sincerity that keeps an apology honest.",
        file: ModelFile { name: "nGzMHMPAWGaJFHOo.safetensors", size: 256136, sha256: "a0c3dcb91b2247eca56536be9ae8e8becc1c807b1136c5fe101552d6a155b24a" },
    },
    PhononVoice {
        name: "Easton",
        locale: "en-us",
        gender: "male",
        age: "Young Adult",
        description: "Punchy and close in, with crisp consonant attack that cuts through a music bed and pitch that lifts across each line.",
        file: ModelFile { name: "LMDIYE0Oa377Qh6Z.safetensors", size: 256136, sha256: "45c582673cb20edb04ddcca2b5a8865ddfb0a82599efadb269021e87d20fbdd4" },
    },
    PhononVoice {
        name: "Edwin",
        locale: "en-gb",
        gender: "male",
        age: "Middle-age",
        description: "Efficient and firm, sincere in attention, with tidy articulation that holds up at phone bandwidth.",
        file: ModelFile { name: "zUCxzrOrugTSis03.safetensors", size: 256136, sha256: "ff1d0735b8b45710462b1d1f846da040f970c6eb62c7c2f36dbfb8e5bde74f1e" },
    },
    PhononVoice {
        name: "Elodie-Rose",
        locale: "en-gb",
        gender: "female",
        age: "Young",
        description: "A young British voice with sharp, enthusiastic energy and crisp articulation, sparky and to the point for customer support and care.",
        file: ModelFile { name: "y9bQqIDnyxwqK01t.safetensors", size: 256136, sha256: "21eade9d83b2e66b9364e1b6cd999a3a6704c000a71f27f34e05ace5cdfd58bb" },
    },
    PhononVoice {
        name: "Everly",
        locale: "en-us",
        gender: "female",
        age: "Middle-age",
        description: "Rounded, soft-edged tone with gentle lift at the end of phrases and an easy, relaxed delivery.",
        file: ModelFile { name: "vjFyp2BcCyTlUmYb.safetensors", size: 256136, sha256: "958ea94da14f46dd48c7f64adac11ed2ad2aa45f7ccd165dfebab5272664402e" },
    },
    PhononVoice {
        name: "Fairy Godmother",
        locale: "characters",
        gender: "",
        age: "",
        description: "A kind fairy godmother, an older woman in her sixties. A low, warm, velvety alto voice, cosy and grandmotherly, unhurried and soothing, with a soft rich chuckle and a twinkle in every word. Gentle and reassuring, never high, never thin.",
        file: ModelFile { name: "QXNr0SvkukLB4wPq.safetensors", size: 256136, sha256: "f390c09f5e279c0f2171995a534b92f5ca5c83443906879da68fc27445f89d89" },
    },
    PhononVoice {
        name: "Fennick the Young Elf",
        locale: "characters",
        gender: "",
        age: "",
        description: "A curious, kind-hearted elf boy of about ten. A natural child's voice in a warm, soft middle range, never squeaky or shrill, earnest and wide-eyed, a little breathless with excitement, speaking quickly when amazed. Clean, close studio recording with no background noise or hiss.",
        file: ModelFile { name: "cXoIXJ2llS8S0WOW.safetensors", size: 256136, sha256: "a05944e4c69d61d10d66f689175ba0b081930df2d77b22f04d00ac40d3d69ecb" },
    },
    PhononVoice {
        name: "Freya",
        locale: "en-gb",
        gender: "female",
        age: "Young",
        description: "A young British voice, glossy and confident with fast, girly chatter and bubbly sparkle, perfect for a friendly receptionist or assistant.",
        file: ModelFile { name: "GgfEkEJtxZR7gnpy.safetensors", size: 256136, sha256: "eb3cd47418349e618408c16bea7153988b3ad04b41a6033039dd474a9a4dcd23" },
    },
    PhononVoice {
        name: "Grub the Ogre",
        locale: "characters",
        gender: "",
        age: "",
        description: "A big, good-natured ogre who lives under a bridge. Gruff and grainy, rumbling low end, heavy chest resonance, slow lumbering rhythm with loose rounded words, and warm humour sitting just under the grumble.",
        file: ModelFile { name: "cYtsRSxV5q9IMehn.safetensors", size: 256136, sha256: "b30fa847686ec1eb6be8c5bea4b6a5c5076ea66c3ff053cf98210a155d8aad1e" },
    },
    PhononVoice {
        name: "Hadley",
        locale: "en-us",
        gender: "female",
        age: "Middle-age",
        description: "Settled, even-toned and genuinely kind, with crisp consonants that hold up over a handset.",
        file: ModelFile { name: "bXwrkHhURL1VFJdG.safetensors", size: 256136, sha256: "a879938aa9a2df0a8220bdadef88137929f71e887bd91e7db205bd5fa11661d2" },
    },
    PhononVoice {
        name: "Harper",
        locale: "en-us",
        gender: "female",
        age: "Adult",
        description: "Modern, confident and friendly voice with a standard American accent.",
        file: ModelFile { name: "4SZHfMpw-p46Ywgs.safetensors", size: 256136, sha256: "6b3da6e2d796a20c51af892c482bd6509fb88267a221aff83c870843dee2a123" },
    },
    PhononVoice {
        name: "Hugh",
        locale: "en-gb",
        gender: "male",
        age: "Middle-age",
        description: "Soft and close, airy in texture, with lively pitch movement and an attentive, sincere delivery.",
        file: ModelFile { name: "YyKcxPG7iIgS3jsK.safetensors", size: 256136, sha256: "3169d74b47a54d1eb15e4c243f56393abe9fe90c7e1c308ecf96ee67618b152a" },
    },
    PhononVoice {
        name: "Imogen",
        locale: "en-gb",
        gender: "female",
        age: "Adult",
        description: "A bubbly, friendly British English adult voice that puts callers instantly at ease. Great for onboarding assistants and warm concierge interactions.",
        file: ModelFile { name: "gDT1nz7Ie36ZhL-C.safetensors", size: 256136, sha256: "1980ab3c35fa98a25b5f3b5e643fd852e14ca8a12a4a26b06aaf1abf2a8c68b1" },
    },
    PhononVoice {
        name: "Juniper",
        locale: "en-us",
        gender: "female",
        age: "Young Adult",
        description: "Cheerful presence with real resonance under it, personable when the copy turns intimate and decisive on the final line.",
        file: ModelFile { name: "NIWIOv5vTi5eRMj8.safetensors", size: 256136, sha256: "444378eb06b7c492b86e8fc7d13e4db1a3c0cf3e68891a960bf6f0a41a13ef35" },
    },
    PhononVoice {
        name: "Lottie",
        locale: "en-gb",
        gender: "female",
        age: "Young Adult",
        description: "Sparky and to the point, high forward energy with clean edges on the consonants and the range to drop low for one line then climb back.",
        file: ModelFile { name: "m9MxpY6IUGXtWp76.safetensors", size: 256136, sha256: "a906ebf17e43fded12b3d6bd077abb2df33bc86eff51c1beeb82e5d416527db3" },
    },
    PhononVoice {
        name: "Maeve",
        locale: "en-gb",
        gender: "female",
        age: "Adult",
        description: "A sparky, attentive British English adult voice that gets to the point with a smile. Ideal for fast, efficient customer support and helpful voice assistants.",
        file: ModelFile { name: "6PWnV0Nq4wu7RVBT.safetensors", size: 256136, sha256: "56494ee67d533ad227ff0dfd0b1c13ff5aa91f1ad6e596e66c2986ba3005e844" },
    },
    PhononVoice {
        name: "Master Aldric",
        locale: "characters",
        gender: "",
        age: "",
        description: "A wise old British wizard, around eighty. Rich and resonant with an aged grain, warm and kindly, deliberate pace with weighted pauses before the words that matter, able to rise into a ringing incantation.",
        file: ModelFile { name: "gSgG5Tor0GPEnqzC.safetensors", size: 256136, sha256: "a83430cc022b85c83753f859f9e44f406651646dab994fced8ef7a9a88005971" },
    },
    PhononVoice {
        name: "Micah",
        locale: "en-us",
        gender: "male",
        age: "Young Adult",
        description: "Buoyant and high in energy, with a playful lift at the ends of phrases and clean attack that survives a loud mix.",
        file: ModelFile { name: "40QrMVgAxFiUnP9f.safetensors", size: 256136, sha256: "0de1098d1051686a138d068e83ff09deb5053328bc74ec9541a1fdb90bfc24da" },
    },
    PhononVoice {
        name: "Miles",
        locale: "en-us",
        gender: "male",
        age: "Young Adult",
        description: "Resonant and decisive, with genuine dynamic range, dropping right down for one clause then driving back up.",
        file: ModelFile { name: "d9XCyhjo4JhkHF1d.safetensors", size: 256136, sha256: "eab20e4f159b48cfbc01b1885804e24f681f80e7a64f909182763a4e3361bee8" },
    },
    PhononVoice {
        name: "Morwenna of the Deep",
        locale: "characters",
        gender: "",
        age: "",
        description: "A sultry, seductive woman in her thirties. Low, husky alto, close to the microphone and almost whispered, slow and languid, purring, with a warm knowing smile in every word. Intimate, flirtatious and teasing, like a late-night torch singer luring someone closer, with a hint of danger underneath.",
        file: ModelFile { name: "nOykTmcH9ydsGGIc.safetensors", size: 256136, sha256: "2ff2db91e70648623df8d42c0dcd7c336b2f14cb4362d602a20f292a4d862154" },
    },
    PhononVoice {
        name: "Old Mortlake",
        locale: "characters",
        gender: "",
        age: "",
        description: "An elderly gravedigger in a Victorian churchyard. Low, raspy and half whispered, melancholic, with heavy pauses that let every word settle like dirt on a coffin lid.",
        file: ModelFile { name: "lX3LavMNdlIbgEHH.safetensors", size: 256136, sha256: "d3f9429205e5906dcd3c78edaf7c9d5148c7e389e34c82c86f72f154adb5d923" },
    },
    PhononVoice {
        name: "Pippa",
        locale: "en-gb",
        gender: "female",
        age: "Adult",
        description: "A confident, upbeat British English adult voice with infectious energy. Perfect for proactive assistants and motivating customer success calls.",
        file: ModelFile { name: "uem82D50GRv2Dwma.safetensors", size: 256136, sha256: "d41a8f01d6e33450f1ce062d332914da10beee99e585dab531750706f187c48a" },
    },
    PhononVoice {
        name: "Preston",
        locale: "en-us",
        gender: "male",
        age: "Middle-age",
        description: "Trim, focused delivery with clean edges on every consonant and a steady, low-set core that carries through phone bandwidth.",
        file: ModelFile { name: "qIxlZ7Ei8KvWlyQH.safetensors", size: 256136, sha256: "6e04b84d3ae8751b22bca0a4f63936f9cb648f4f2182b2d0788071ea1c0b68a1" },
    },
    PhononVoice {
        name: "Russell",
        locale: "en-us",
        gender: "male",
        age: "Adult",
        description: "A high-energy American adult voice that pushes and encourages with the intensity of someone who genuinely believes in you.",
        file: ModelFile { name: "_6Aslh2DxfmnRLmP.safetensors", size: 256136, sha256: "beae9e4030efce0a935fcb0f807dc0784d598bcaa87ad4aacb047436b1eaa411" },
    },
    PhononVoice {
        name: "Sir Bramwell",
        locale: "characters",
        gender: "",
        age: "",
        description: "A brave but pompous English knight, middle-aged. Booming, plummy and theatrical, full of puffed-up self-importance, rolling grand phrases, with a comic wobble of nerves showing through the bravado.",
        file: ModelFile { name: "y2eiKxtQo2B67deU.safetensors", size: 256136, sha256: "b3cea6789aeb58822af16ba08c4e5105d7febb840dcb67b47f426c19d1a8f209" },
    },
    PhononVoice {
        name: "Spencer",
        locale: "en-gb",
        gender: "male",
        age: "Middle-age",
        description: "Light and airy, sitting a touch higher, with crisp consonants and a slow, patient delivery.",
        file: ModelFile { name: "JOW0vchyswkQUMDS.safetensors", size: 256136, sha256: "cc9ac1107b0ffce282374a19beb7bfa95847724e6b7afbac991b4ae4277d5e6d" },
    },
    PhononVoice {
        name: "Teddy",
        locale: "en-gb",
        gender: "male",
        age: "Young Adult",
        description: "Sparky and close in, quick off the mark, with firm consonants and the range to drop right down for a clause then come back up.",
        file: ModelFile { name: "Zn7g6NxLqXlJb7Qi.safetensors", size: 256136, sha256: "baea0ea9506f3237a5ebfc61afb7c72dbd189c5603b199ef42fbb00dbda379e0" },
    },
    PhononVoice {
        name: "The Snow Queen",
        locale: "characters",
        gender: "",
        age: "",
        description: "A regal, cold-hearted queen of ice. A cool, crystalline mezzo voice, poised and elegant, precise articulation, unhurried and quietly commanding, with an enigmatic chill and a faint smile that never warms.",
        file: ModelFile { name: "NIWoEurJHMJjyUTe.safetensors", size: 256136, sha256: "bc8a8fcb95bb4f32d2d4376aa69d81b186b3ba2f8e9b6cb6a403b2b194590e85" },
    },
    PhononVoice {
        name: "The Thing Under the Stairs",
        locale: "characters",
        gender: "",
        age: "",
        description: "A monstrous creature lurking in the dark. Gravelly, breathy and pitched near the floor, a threatening growl that stays quiet to stay frightening, with a slow mocking sneer.",
        file: ModelFile { name: "GXaSqMEmOg12OIYb.safetensors", size: 256136, sha256: "bfa37bac98aefbb9de80f792c47d01a0f9e3b94e73776169040c249c8b722759" },
    },
    PhononVoice {
        name: "Toby",
        locale: "en-gb",
        gender: "male",
        age: "Adult",
        description: "A sparky, attentive British English adult voice that gets to the point with a smile. Ideal for fast, efficient customer support and helpful voice assistants.",
        file: ModelFile { name: "dME3IWyZBvmh1n1q.safetensors", size: 256136, sha256: "09ab315c7f4b72b4d71e66cb756184cedfb1f14d222d096e115bf95bfe7b29dc" },
    },
    PhononVoice {
        name: "Trevor",
        locale: "en-us",
        gender: "male",
        age: "Middle-age",
        description: "Buoyant energy over a resonant mid-low core, phrases landing softly and consonants cutting through a handset.",
        file: ModelFile { name: "0zLApsZfVLLNaEGl.safetensors", size: 256136, sha256: "dc1341c8937af5534e6bbcc5c0fc5e7c10312229b59061228074f93683fe2261" },
    },
    PhononVoice {
        name: "Vyrmathrax the Ancient",
        locale: "characters",
        gender: "",
        age: "",
        description: "An ancient dragon, thousands of years old. Very low and resonant, deep chest weight with gravel at the bottom of the register, slow and controlled with long pauses, dry and offhand, with the flat calm of something far too old to be impressed.",
        file: ModelFile { name: "xO8xQo3BrB8FuI6J.safetensors", size: 256136, sha256: "63ab4b15427844a804480a1603774262e8903dee33cb279e2749860dbfaedd10" },
    },
    PhononVoice {
        name: "Zoey",
        locale: "en-us",
        gender: "female",
        age: "Adult",
        description: "Playful, upbeat and Gen Z energy voice with a standard American accent. Perfect for engaging conversations!",
        file: ModelFile { name: "NbpkqMVS3CJeq2j8.safetensors", size: 256136, sha256: "97c7f8c253413574406ab80dd23f49c3ebb2517163b00a9bf86b9c4cf43e7e23" },
    },
];
