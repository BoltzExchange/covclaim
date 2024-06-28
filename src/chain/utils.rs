use std::error::Error;

pub fn parse_hex<T: elements::encode::Decodable>(hex_str: String) -> Result<T, Box<dyn Error>> {
    match elements::encode::deserialize(
        match hex::decode(hex_str) {
            Ok(res) => res,
            Err(err) => return Err(Box::new(err)),
        }
        .as_ref(),
    ) {
        Ok(block) => Ok(block),
        Err(e) => Err(Box::new(e)),
    }
}
