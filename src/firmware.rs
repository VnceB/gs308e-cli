use crate::admin::{Change, attr, css, input, one, token};
use crate::error::{Error, Result};
use crate::model;
use crate::session::{Page, Session};
use scraper::Html;
use std::{fs::File, io::Read, path::Path};

use crate::profile::{REVIEWED as TARGET, parse_version};
const IMAGE_BYTES: usize = 1_316_304;
const SHA256: &str = "db92d85448ada73d4e094d0c623340f3b86cbb04272a64ac06e1a5851037b9ae";
const NAME: &str = "GS308Ev4_V1.0.1.13.bin";

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() + 8) % 64 != 0 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (state, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(delta);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

fn form(page: &Html) -> Result<(usize, usize, String)> {
    let root = page.root_element();
    let form = one(root, "form#uploadForm")?;
    if !attr(form, "method")?.eq_ignore_ascii_case("post")
        || attr(form, "enctype")? != "multipart/form-data"
        || attr(form, "action")? != "/firmware.cgi?cmd=fw_upgrade"
        || input(root, "input#prctName")? != "GS308Ev4"
        || form.select(&css("[name]")).count() != 2
    {
        return Err(Error::Page("unsupported firmware upload form"));
    }
    one(form, "input[type=file][name=fileField][accept='.bin']")?;
    one(form, "input[type=hidden][name=hash]#hash")?;
    let kb = |key| {
        input(root, key)?
            .parse::<usize>()
            .ok()
            .and_then(|x| x.checked_mul(1024))
            .ok_or(Error::Page("firmware upload limits"))
    };
    Ok((kb("#minFileSize")?, kb("#maxFileSize")?, token(page)?))
}
fn multipart(image: &[u8], key: &str) -> Result<(String, Vec<u8>)> {
    let boundary = "gs308e-1700a0644d820827-binary";
    if key.contains(boundary)
        || image
            .windows(boundary.len())
            .any(|s| s == boundary.as_bytes())
    {
        return Err(Error::Usage("multipart delimiter collision"));
    }
    let mut body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"hash\"\r\n\r\n{key}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"fileField\"; filename=\"{NAME}\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend_from_slice(image);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}
pub fn upgrade(
    session: &Session<'_>,
    path: &Path,
    apply: bool,
    protected: &[u8],
    dashboard: &Html,
) -> Result<Change> {
    if apply && !protected.is_empty() {
        return Err(Error::Usage("firmware installation interrupts all ports"));
    }
    let installed = model::device(dashboard)?.firmware;
    let current = parse_version(&installed).ok_or(Error::Page("installed firmware version"))?;
    if current > TARGET {
        return Err(Error::Usage("firmware downgrade is unsupported"));
    }
    let ui = form(&session.read(Page::Firmware)?)?;
    if !(ui.0..=ui.1).contains(&IMAGE_BYTES) {
        return Err(Error::Page(
            "approved firmware exceeds device upload bounds",
        ));
    }
    let file =
        File::open(path).map_err(|_| Error::Usage("unable to open the firmware image file"))?;
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() == IMAGE_BYTES as u64)
    {
        return Err(Error::Usage(
            "expected the extracted GS308Ev4 V1.0.1.13 image",
        ));
    }
    let mut bytes = Vec::with_capacity(IMAGE_BYTES);
    file.take(IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Usage("firmware file read failed"))?;
    let digest = sha256_hex(&bytes);
    if bytes.len() != IMAGE_BYTES || digest != SHA256 {
        return Err(Error::Usage(
            "firmware digest differs from the reviewed NETGEAR distribution",
        ));
    }
    let outcome = if current == TARGET {
        "unchanged"
    } else if apply {
        let fresh = session.read(Page::Dashboard)?;
        model::check_model(&fresh)?;
        if model::device(&fresh)?.firmware != installed {
            return Err(Error::Usage("installed firmware changed during preflight"));
        }
        let (min, max, key) = form(&session.read(Page::Firmware)?)?;
        if (min, max) != (ui.0, ui.1) {
            return Err(Error::Usage("firmware form changed during preflight"));
        }
        let (content, data) = multipart(&bytes, &key)?;
        session.upload_firmware(&content, data)?;
        "accepted"
    } else {
        "preview"
    };
    Ok(Change {
        operation: "firmware-upgrade",
        outcome,
        details: format!(
            "{installed} -> V1.0.1.13; confirm the installed version with info after the switch restarts"
        ),
    })
}
