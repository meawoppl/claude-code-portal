use yew::prelude::*;

#[derive(Debug, Clone)]
pub enum DiffLine<'a> {
    Context(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

/// Generate diff view HTML from old and new strings
pub fn render_diff_lines(old_string: &str, new_string: &str) -> Html {
    let old_lines: Vec<&str> = old_string.lines().collect();
    let new_lines: Vec<&str> = new_string.lines().collect();

    let diff = compute_line_diff(&old_lines, &new_lines);

    html! {
        <div class="diff-view">
            {
                diff.iter().map(|change| {
                    match change {
                        DiffLine::Context(line) => html! {
                            <div class="diff-line context">
                                <span class="diff-marker">{ " " }</span>
                                <span class="diff-content">{ *line }</span>
                            </div>
                        },
                        DiffLine::Removed(line) => html! {
                            <div class="diff-line removed">
                                <span class="diff-marker">{ "-" }</span>
                                <span class="diff-content">{ *line }</span>
                            </div>
                        },
                        DiffLine::Added(line) => html! {
                            <div class="diff-line added">
                                <span class="diff-marker">{ "+" }</span>
                                <span class="diff-content">{ *line }</span>
                            </div>
                        },
                    }
                }).collect::<Html>()
            }
        </div>
    }
}

/// Compute a line-based diff between old and new content
fn compute_line_diff<'a>(old_lines: &[&'a str], new_lines: &[&'a str]) -> Vec<DiffLine<'a>> {
    let lcs = longest_common_subsequence(old_lines, new_lines);

    let mut result = Vec::new();
    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut lcs_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        if lcs_idx < lcs.len() {
            let (lcs_old, lcs_new) = lcs[lcs_idx];

            while old_idx < lcs_old {
                result.push(DiffLine::Removed(old_lines[old_idx]));
                old_idx += 1;
            }

            while new_idx < lcs_new {
                result.push(DiffLine::Added(new_lines[new_idx]));
                new_idx += 1;
            }

            result.push(DiffLine::Context(old_lines[old_idx]));
            old_idx += 1;
            new_idx += 1;
            lcs_idx += 1;
        } else {
            while old_idx < old_lines.len() {
                result.push(DiffLine::Removed(old_lines[old_idx]));
                old_idx += 1;
            }
            while new_idx < new_lines.len() {
                result.push(DiffLine::Added(new_lines[new_idx]));
                new_idx += 1;
            }
        }
    }

    result
}

/// Compute longest common subsequence indices for line diff
fn longest_common_subsequence(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();

    if m == 0 || n == 0 {
        return Vec::new();
    }

    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    result.reverse();
    result
}
