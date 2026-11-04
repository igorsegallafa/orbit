//! Fuzzy ranking of Search Everywhere (`src/lib/fuzzy.ts`).

/// Subsequence score: longer runs of consecutive matches and matches at word
/// boundaries score higher. 0 = no match. Expects lower case.
pub fn fuzzy_score(s: &str, q: &str) -> u32 {
    if s == q {
        return 1000;
    }
    let s: Vec<char> = s.chars().collect();
    let mut si = 0;
    let mut score = 0;
    let mut streak = 0;
    for ch in q.chars() {
        let Some(idx) = s.get(si..).and_then(|rest| rest.iter().position(|&c| c == ch)).map(|p| si + p) else { return 0 };
        streak = if idx == si { streak + 1 } else { 0 };
        score += 1 + streak * 2;
        if idx == 0 || "/-._ ".contains(s[idx - 1]) {
            score += 3;
        }
        si = idx + 1;
    }
    score
}

/// Items ranked for a query by path: the file name counts more than its folders.
pub fn rank_paths<T: Clone>(items: &[T], path: impl Fn(&T) -> String, query: &str, limit: usize) -> Vec<T> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return items.iter().take(limit).cloned().collect();
    }
    let mut scored: Vec<(u32, usize, &T)> = items
        .iter()
        .filter_map(|it| {
            let p = path(it).to_lowercase();
            let name = p.rsplit('/').next().unwrap_or(&p);
            let s = fuzzy_score(&p, &q);
            (s > 0).then(|| (s + fuzzy_score(name, &q) * 2 + name_bonus(name, &q), name.len(), it))
        })
        .collect();
    // Best score first; among equals the shorter name, then the listing order (stable sort).
    scored.sort_by_key(|a| (std::cmp::Reverse(a.0), a.1));
    scored.into_iter().take(limit).map(|(_, _, it)| it.clone()).collect()
}

/// "game" should find `Game.cpp` before `gameserver.proto`.
fn name_bonus(name: &str, q: &str) -> u32 {
    let stem = name.split('.').next().unwrap_or(name);
    if stem == q || name == q {
        500
    } else if name.starts_with(q) {
        100
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequences_and_rejects_the_rest() {
        assert!(fuzzy_score("src/game/game.cpp", "gamecpp") > 0);
        assert_eq!(fuzzy_score("src/game/game.cpp", "xyz"), 0);
        assert_eq!(fuzzy_score("abc", "abc"), 1000);
        assert_eq!(fuzzy_score("ab", "abc"), 0);
    }

    #[test]
    fn ranks_a_matching_file_name_above_a_matching_folder() {
        let paths = ["src/game/Hud/HudWidget.cpp", "src/game/Game.cpp", "src/gameserver/Main.cpp"].map(String::from);
        assert_eq!(rank_paths(&paths, |p| p.clone(), "game.cpp", 3)[0], "src/game/Game.cpp");
    }

    #[test]
    fn ranks_an_exact_file_name_above_longer_ones() {
        let paths = ["proto/gameserver.proto", "src/Engine/GameLoop.cpp", "src/game/Game.cpp", "src/game/Game.h"].map(String::from);
        let ranked = rank_paths(&paths, |p| p.clone(), "Game", 4);
        assert_eq!(&ranked[..2], ["src/game/Game.h", "src/game/Game.cpp"]);
    }

    #[test]
    fn empty_query_keeps_order_up_to_the_limit() {
        let paths = ["b", "a", "c"].map(String::from);
        assert_eq!(rank_paths(&paths, |p| p.clone(), "  ", 2), ["b", "a"]);
    }
}
