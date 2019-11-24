use sprs::*;
use std::collections::vec_deque::VecDeque;
use std::collections::HashMap;
use vtext::tokenize::Tokenizer;
use vtext::vectorize::{CountVectorizer, CountVectorizerParams};

pub struct VecMatcher {
    vectorizer: CountVectorizer<Ngram>,
    ngram: Ngram,
    mat: CsMat<f32>,
    row_norms: Vec<f32>,
    workspace: Vec<i32>,
    texts: Vec<String>,
}

impl VecMatcher {
    pub fn new(texts: &[String], arity: usize) -> Self {
        // Pad texts
        let arity = arity.max(1);
        let ngram = Ngram::new(arity);
        let mut storage = Vec::new();
        for text in texts {
            storage.push(ngram.pad_str(text));
        }

        // Build model
        let mut vectorizer = CountVectorizerParams::default()
            .tokenizer(ngram.clone())
            .build()
            .unwrap();
        let mat = vectorizer.fit_transform(&storage);

        // Precompute norms. Empty strings has been padded, so all norms are positive
        let norms = mat
            .outer_iterator()
            .map(|vec| {
                let tmp: f32 = vec.iter().map(|(_, x)| (x * x) as f32).sum();
                tmp.sqrt()
            })
            .collect();
        // IDF
        let mut cmat = mat.map(|&x| x as f32).to_csc();
        // for mut col in cmat.outer_iterator_mut() {
        //     if col.nnz() > 0 {
        //         let idf = (col.dim() as f32 / col.nnz() as f32);
        //         assert!(idf >= 1.0);
        //         col.map_inplace(|&x| x * idf.ln());
        //     }
        // }

        Self {
            vectorizer: vectorizer,
            workspace: vec![0i32; mat.rows()],
            mat: cmat,
            row_norms: norms,
            ngram: ngram,
            texts: storage,
        }
    }

    pub fn get_text(&self, index: usize) -> &str {
        return self.ngram.unpad_str(&self.texts[index]);
    }

    #[inline]
    fn compute_prob<'a>(&mut self, padded_text: String) -> CsMat<f32> {
        let mat = self.vectorizer.transform(&[padded_text]).map(|&x| x as f32);
        assert!(self.mat.is_csc());
        assert!(mat.transpose_view().is_csc());
        &self.mat * &mat.transpose_view()
    }

    fn compute_norm(&self, padded_text: &str) -> f32 {
        let mut token_hash = HashMap::new();
        let padded_text = padded_text.to_ascii_lowercase();
        for tok in self.ngram.tokenize(&padded_text) {
            let count = token_hash.entry(tok).or_insert(0);
            *count += 1;
        }
        // Because we have padded text norm is positive
        let tmp: f32 = token_hash.values().map(|&c| (c * c) as f32).sum();
        tmp.sqrt()
    }

    pub fn search_best(&mut self, text: &str, threshold: f32) -> Option<(usize, f32)> {
        let s = self.ngram.pad_str(text);
        let norm = self.compute_norm(&s);

        let m = self.compute_prob(s);
        let prob = m.outer_view(0).unwrap();
        assert_eq!(prob.dim(), self.mat.rows());

        use std::cmp::Ordering;
        if let Some((i, val)) = prob
            .iter()
            .map(|(i, &val)| (i, val as f32 / norm / self.row_norms[i]))
            .max_by(|(_, x), (_, y)| {
                // Ignore NaN by makeing it always less
                x.partial_cmp(y).unwrap_or(if x.is_nan() {
                    Ordering::Less
                } else {
                    Ordering::Greater
                })
            })
        {
            if val >= threshold {
                Some((i, val))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn search(&mut self, text: &str, threshold: f32, nbest: usize) -> Vec<(usize, f32)> {
        let s = self.ngram.pad_str(text);
        let norm = self.compute_norm(&s);

        let m = self.compute_prob(s);
        let prob = m.outer_view(0).unwrap();
        assert_eq!(prob.dim(), self.mat.rows());

        // TODO: find top n can be done faster than sorting all
        let mut v = prob
            .iter()
            .map(|(i, &val)| (i, val as f32 / norm / self.row_norms[i]))
            .filter(|&(_, val)| val > threshold)
            .collect::<Vec<_>>();
        use std::cmp::Ordering;
        v.sort_by(|(_, x), (_, y)| {
            // Ignore NaN by makeing it always less
            x.partial_cmp(y).unwrap_or(if x.is_nan() {
                Ordering::Less
            } else {
                Ordering::Greater
            })
        });
        v.into_iter().rev().take(nbest).collect()
    }
}

struct CharWindows<'a> {
    window: usize,
    text: &'a str,
    stored: VecDeque<usize>,
    iter: std::str::CharIndices<'a>,
}

impl<'a> CharWindows<'a> {
    fn new(text: &'a str, window: usize) -> Self {
        let b = VecDeque::new();
        Self {
            window: window.max(1),
            text: text,
            stored: b,
            iter: text.char_indices(),
        }
    }
}

impl<'a> Iterator for CharWindows<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next() {
                Some((pos, c)) => {
                    self.stored.push_back(pos);
                    if self.stored.len() >= self.window {
                        let begin = self.stored.pop_front().unwrap();
                        return Some(&self.text[begin..pos + c.len_utf8()]);
                    }
                }
                None => return None,
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Ngram {
    window: usize,
}

impl Ngram {
    fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
        }
    }
    fn pad_str(&self, text: &str) -> String {
        let pad = self.window - 1;
        let mut s = String::with_capacity(text.len() + pad * 2);
        for _ in 0..pad {
            s.push(' ');
        }
        s.push_str(text);
        for _ in 0..pad {
            s.push(' ')
        }
        s
    }
    fn unpad_str<'a>(&self, padded_text: &'a str) -> &'a str {
        let pad = self.window - 1;
        &padded_text[pad..padded_text.len() - pad]
    }
}

impl Default for Ngram {
    fn default() -> Self {
        Self { window: 2 }
    }
}

impl Tokenizer for Ngram {
    fn tokenize<'a>(&'a self, text: &'a str) -> Box<dyn Iterator<Item = &'a str> + 'a> {
        // FIXME: data cleaning should be in the other place
        Box::new(
            CharWindows::new(text, self.window).filter(|&s| !s.contains('(') && !s.contains(')')),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_approx_eq::assert_approx_eq;

    #[test]
    fn ngram() {
        let ngram = Ngram::new(3);
        assert_eq!(ngram.pad_str("Cats"), "  Cats  ".to_owned());
        assert_eq!(ngram.unpad_str(&ngram.pad_str("Dogs")), "Dogs");
    }

    #[test]
    fn windows() {
        assert_eq!(
            CharWindows::new("abcdefg", 2).collect::<Vec<_>>(),
            vec!["ab", "bc", "cd", "de", "ef", "fg"]
        );
        assert_eq!(
            CharWindows::new("abcdefg", 4).collect::<Vec<_>>(),
            vec!["abcd", "bcde", "cdef", "defg"]
        );
        assert_eq!(
            CharWindows::new("abc", 1).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert!(CharWindows::new("abc", 4).collect::<Vec<_>>().is_empty());
    }

    #[test]
    fn check_search() {
        let dataset = vec!["Animal Planet HD".to_owned()];
        let mut corpus = VecMatcher::new(&dataset, 2);
        dbg!(corpus.mat.to_dense());
        dbg!(&corpus.row_norms);
        let (i, sim) = corpus.search_best(&dataset[0], 0.9).unwrap();
        assert_eq!(i, 0);
        assert_approx_eq!(sim, 1., 1e-3);
    }
}
