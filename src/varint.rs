/// Parses minecraft's variable-length ints from async streams
/// Derived from https://github.com/luojia65/mc-varint
use std::marker::Unpin;
use tokio::io::{AsyncRead, AsyncReadExt};
pub async fn read_var_i64<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<i64> {
    let mut buf = [0];
    let mut ans = 0;
    for i in 0_u8..8 {
        r.read_exact(&mut buf).await?;
        ans |= ((buf[0] & 0b0111_1111) as i64) << 7 * i;
        if buf[0] & 0b1000_0000 == 0 {
            break;
        }
    }
    Ok(ans)
}
