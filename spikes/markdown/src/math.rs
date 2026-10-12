//! RaTeX: does it typeset what agents write, and how fast?

use std::time::Instant;

use pulldown_cmark::{Event, Parser};
use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_types::display_item::DisplayList;

use crate::{corpus, pct, stream};

/// A spread of what KaTeX-style math looks like in technical writing.
pub const FIXTURES: &[&str] = &[
    r"E = mc^2",
    r"\frac{a}{b} + \frac{1}{1 + \frac{1}{x}}",
    r"\sqrt{x^2 + y^2} \quad \sqrt[3]{8}",
    r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
    r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
    r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
    r"\prod_{p \text{ prime}} \frac{1}{1 - p^{-s}}",
    r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
    r"\begin{bmatrix} 1 & 0 & 0 \\ 0 & 1 & 0 \\ 0 & 0 & 1 \end{bmatrix}",
    r"\begin{aligned} f(x) &= (x+1)^2 \\ &= x^2 + 2x + 1 \end{aligned}",
    r"f(x) = \begin{cases} 0 & x < 0 \\ 1 & x \ge 0 \end{cases}",
    r"\left( \frac{a}{b} \right)^{2} \left\{ x \middle| x > 0 \right\}",
    r"\hat{x} \bar{y} \vec{v} \dot{a} \tilde{n} \widehat{abc}",
    r"\mathbb{R}^n \mathcal{O}(n \log n) \mathfrak{g} \mathbf{v}",
    r"\alpha \beta \gamma \Delta \Omega \varepsilon \phi \varphi",
    r"a \le b \ne c \approx d \equiv e \pmod{n}",
    r"\forall x \in A,\ \exists y \notin B : x \subseteq y",
    r"\overbrace{1 + 2 + \cdots + n}^{n} \underbrace{a+b}_{c}",
    r"\binom{n}{k} = \frac{n!}{k!(n-k)!}",
    r"\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}",
    r"\boxed{x = 42}",
    r"\color{red} x + \textcolor{blue}{y}",
    r"\xrightarrow[\text{below}]{\text{above}}",
    r"\ce{H2O + CO2 -> H2CO3}",
    r"\operatorname{softmax}(z)_i = \frac{e^{z_i}}{\sum_j e^{z_j}}",
    r"P(A \mid B) = \frac{P(B \mid A)\,P(A)}{P(B)}",
    r"\mathrm{Attention}(Q,K,V) = \mathrm{softmax}\!\left(\frac{QK^\top}{\sqrt{d_k}}\right)V",
    r"x_{1,2} = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
    r"\| x \|_2 = \sqrt{\sum_i x_i^2}",
    r"\underset{\theta}{\arg\min}\; \mathcal{L}(\theta)",
    r"\begin{array}{c|c} a & b \\ \hline c & d \end{array}",
    r"\text{if } n \text{ is even}",
    r"\iint_D f\,dA \oint_C \mathbf{F}\cdot d\mathbf{r}",
    r"\lfloor x \rfloor \lceil y \rceil \langle u, v \rangle",
    r"\overline{AB} \underline{x} \overrightarrow{AB}",
    r"\stackrel{?}{=} \overset{\text{def}}{=} \coloneqq",
    r"\begin{vmatrix} a & b \\ c & d \end{vmatrix} = ad - bc",
    r"\begin{gather} a = b \\ c = d \end{gather}",
    r"\tag{1} y = mx + b",
    r"\href{https://example.com}{x}",
];

pub struct Typeset {
    pub list: DisplayList,
    pub us: f64,
}

pub fn typeset(src: &str, display: bool) -> Result<Typeset, String> {
    let t = Instant::now();
    let nodes = ratex_parser::parse(src).map_err(|e| format!("{e:?}"))?;
    let opts = LayoutOptions {
        style: if display {
            ratex_types::math_style::MathStyle::Display
        } else {
            ratex_types::math_style::MathStyle::Text
        },
        ..LayoutOptions::default()
    };
    let list = to_display_list(&layout(&nodes, &opts));
    Ok(Typeset {
        list,
        us: t.elapsed().as_secs_f64() * 1e6,
    })
}

pub fn run(msgs: &[corpus::Message], mode: Option<&str>) {
    // Fixtures: which constructs RaTeX handles.
    let mut times = Vec::new();
    let mut failed = Vec::new();
    for f in FIXTURES {
        match typeset(f, true) {
            Ok(t) => {
                if mode == Some("--fixtures") {
                    let red = t
                        .list
                        .items
                        .iter()
                        .any(|i| format!("{i:?}").contains("r: 0.8"));
                    println!(
                        "  {:6.0} µs {} items{} {f}",
                        t.us,
                        t.list.items.len(),
                        if red { " RED" } else { "" }
                    );
                }
                times.push(t.us)
            }
            Err(e) => failed.push(format!(
                "{f}  →  {}",
                e.chars().take(100).collect::<String>()
            )),
        }
    }
    println!(
        "fixtures: {}/{} typeset, p50 {:.0} µs, max {:.0} µs (cold first: {:.0} µs)",
        times.len(),
        FIXTURES.len(),
        pct(&mut times.clone(), 50.),
        pct(&mut times.clone(), 100.),
        times.first().copied().unwrap_or(0.)
    );
    for f in &failed {
        println!("  failed: {f}");
    }

    // Corpus: every span the parser recognises as math.
    let (mut inline, mut display, mut ok, mut bad) = (0, 0, 0, 0);
    let mut times = Vec::new();
    let mut bad_samples = Vec::new();
    for m in msgs {
        for ev in Parser::new_ext(&m.text, stream::options()).map(stream::dollars) {
            let (src, d) = match ev {
                Event::InlineMath(s) => (s, false),
                Event::DisplayMath(s) => (s, true),
                _ => continue,
            };
            if d {
                display += 1
            } else {
                inline += 1
            }
            if mode == Some("--spans") {
                println!("  span {:?}", src.chars().take(70).collect::<String>());
            }
            match typeset(&src, d) {
                Ok(t) => {
                    ok += 1;
                    times.push(t.us);
                }
                Err(e) => {
                    bad += 1;
                    bad_samples.push(format!(
                        "{src:?} → {}",
                        e.chars().take(80).collect::<String>()
                    ));
                }
            }
        }
    }
    println!(
        "corpus: {inline} inline + {display} display spans, {ok} typeset, {bad} failed; p50 {:.0} µs, p99 {:.0} µs, max {:.0} µs",
        pct(&mut times.clone(), 50.),
        pct(&mut times.clone(), 99.),
        pct(&mut times, 100.)
    );
    // Private: real spans, local debugging only.
    if mode == Some("--show") {
        for s in bad_samples.iter().take(60) {
            println!("  {s}");
        }
    }
}
