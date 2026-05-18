use syn::{Block, Expr, Item, Stmt};

const REPLAY_GUARD_IDENT: &str = "__AUTUMN_IDEMPOTENCY_REPLAY_GUARD";

pub fn block_has_replay_guard(block: &Block) -> bool {
    let has_marker = block.stmts.iter().any(|stmt| {
        matches!(
            stmt,
            Stmt::Item(Item::Const(item)) if item.ident == REPLAY_GUARD_IDENT
        )
    });

    has_marker && block_calls_replay_response(block)
}

fn block_calls_replay_response(block: &Block) -> bool {
    block.stmts.iter().any(stmt_calls_replay_response)
}

fn stmt_calls_replay_response(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => expr_calls_replay_response(expr),
        Stmt::Local(syn::Local {
            init: Some(init), ..
        }) => expr_calls_replay_response(&init.expr),
        Stmt::Item(_) | Stmt::Macro(_) | Stmt::Local(_) => false,
    }
}

fn expr_calls_replay_response(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => {
            path_expr_ends_with(&call.func, "__replay_response")
                || call.args.iter().any(expr_calls_replay_response)
        }
        Expr::If(expr_if) => {
            expr_calls_replay_response(&expr_if.cond)
                || block_calls_replay_response(&expr_if.then_branch)
                || expr_if
                    .else_branch
                    .as_ref()
                    .is_some_and(|(_, else_expr)| expr_calls_replay_response(else_expr))
        }
        Expr::Let(expr_let) => expr_calls_replay_response(&expr_let.expr),
        Expr::Block(expr_block) => block_calls_replay_response(&expr_block.block),
        Expr::Async(expr_async) => block_calls_replay_response(&expr_async.block),
        Expr::Await(expr_await) => expr_calls_replay_response(&expr_await.base),
        Expr::Match(expr_match) => {
            expr_calls_replay_response(&expr_match.expr)
                || expr_match
                    .arms
                    .iter()
                    .any(|arm| expr_calls_replay_response(&arm.body))
        }
        _ => false,
    }
}

fn path_expr_ends_with(expr: &Expr, expected: &str) -> bool {
    let Expr::Path(path) = expr else {
        return false;
    };

    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == expected)
}

#[cfg(test)]
mod tests {
    use super::block_has_replay_guard;

    #[test]
    fn string_literal_does_not_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            let _ = "__AUTUMN_IDEMPOTENCY_REPLAY_GUARD";
        });

        assert!(!block_has_replay_guard(&block));
    }

    #[test]
    fn marker_const_without_replay_call_does_not_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            const __AUTUMN_IDEMPOTENCY_REPLAY_GUARD: () = ();
            let _ = "plain user const";
        });

        assert!(!block_has_replay_guard(&block));
    }

    #[test]
    fn generated_const_and_replay_call_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            const __AUTUMN_IDEMPOTENCY_REPLAY_GUARD: () = ();
            if let ::core::option::Option::Some(__autumn_response) =
                ::autumn_web::idempotency::__replay_response(&__autumn_idempotency_replay)
            {
                return __autumn_response;
            }
        });

        assert!(block_has_replay_guard(&block));
    }
}
