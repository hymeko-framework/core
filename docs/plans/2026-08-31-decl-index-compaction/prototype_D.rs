use std::hint::black_box; use std::time::Instant;
const N: usize = 1_000_000; const SENT: u64 = 0; // 0 = NONE; stored value = (kind<<62)|(index+1)
const NODE:u64=0; const EDGE:u64=1; const ARC:u64=2;
fn kind_of(i:usize)->u64{ match i%5 {0|1=>NODE,2|3=>EDGE,_=>ARC} }
#[inline] fn enc(kind:u64, idx:usize)->u64{ (kind<<62)|((idx as u64)+1) } // +1 so 0 stays NONE
struct RepA{ d2n:Vec<Option<usize>> } // baseline single map
impl RepA{ #[inline] fn as_node(&self,d:usize)->Option<usize>{ self.d2n.get(d).copied().flatten() } }
struct RepD{ d2r:Vec<u64> }
impl RepD{ #[inline] fn as_node(&self,d:usize)->Option<usize>{
  let r=self.d2r[d]; if r!=SENT && (r>>62)==NODE { Some(((r & ((1u64<<62)-1))-1) as usize) } else { None } } }
fn median(mut v:Vec<f64>)->f64{ v.sort_by(|a,b|a.partial_cmp(b).unwrap()); v[v.len()/2] }
fn t<F:FnMut()->usize>(reps:usize,mut f:F)->f64{ for _ in 0..3{black_box(f());} let mut s=Vec::new();
  for _ in 0..reps{ let x=Instant::now(); let a=f(); s.push(x.elapsed().as_secs_f64()*1e9); black_box(a);} median(s) }
fn main(){
  let mut a=RepA{d2n:vec![None;N]}; let mut d=RepD{d2r:vec![SENT;N]};
  let mut nc=0usize;
  for i in 0..N{ let k=kind_of(i); match k{ NODE=>{a.d2n[i]=Some(nc); d.d2r[i]=enc(NODE,nc); nc+=1;}
    EDGE=>{ d.d2r[i]=enc(EDGE, i); } _=>{ d.d2r[i]=enc(ARC, i); } } }
  println!("D bytes/decl = {:.1} (u64); reduction vs 48 = {:.2}x", 8.0, 48.0/8.0);
  let seq=|f:&dyn Fn(usize)->Option<usize>|->usize{ (0..N).map(|x|f(x).unwrap_or(0)).sum() };
  let mut idx:Vec<usize>=(0..N).collect(); let mut s=0x1234_5678u64;
  for i in (1..N).rev(){ s=s.wrapping_mul(6364136223846793005).wrapping_add(1); let j=(s>>11)as usize%(i+1); idx.swap(i,j);}
  let rnd=|f:&dyn Fn(usize)->Option<usize>|->usize{ idx.iter().map(|&x|f(x).unwrap_or(0)).sum() };
  for pass in 0..2 {
    let sa=t(15,||seq(&|x|a.as_node(x))); let sd=t(15,||seq(&|x|d.as_node(x)));
    let ra=t(15,||rnd(&|x|a.as_node(x))); let rd=t(15,||rnd(&|x|d.as_node(x)));
    println!("pass{}: SEQ A={:.0} D={:.0} ({:+.1}%) | RND A={:.0} D={:.0} ({:+.1}%)",
      pass, sa, sd,(sd-sa)/sa*100.0, ra, rd,(rd-ra)/ra*100.0);
  }
}
