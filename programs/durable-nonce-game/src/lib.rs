use anchor_lang::{
    prelude::*,
    solana_program::native_token::LAMPORTS_PER_SOL, 
    system_program::{transfer, Transfer}
};
declare_id!("6jQwFBu5dxZqRayAdUJ2iCmCupGLMKhHUDYqSQghMWqj");

#[program]
pub mod durable_nonce_game {
    use super::*;

    pub fn create_game(ctx: Context<CreateGame>) -> Result<()> {

        // Create the game account
        ctx.accounts.game.set_inner(
            Game {
                state: GameState::Pending,
                board: [0; 9],
                player_one: ctx.accounts.player.key(),
                player_two: Pubkey::default(),
                last_update: 0,
                bump: ctx.bumps.game,
                vault_bump: ctx.bumps.vault,
            }
        );

        // Create the bet
        transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.player.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                }
            ),
            1 * LAMPORTS_PER_SOL,
        )?;

        Ok(())
    }

    pub fn accept_game(ctx: Context<AcceptGame>, board: [u8; 9] ) -> Result<()> {

        match ctx.accounts.game.state {
            GameState::Pending => {
                require_neq!(ctx.accounts.player_two.key(), ctx.accounts.game.player_one, GameError::CannotPlayAgainstYourself);
            }
            _ => return Err(GameError::AlreadyStarted.into()),
        }
        // Change the game state to PlayerOneTurn
        ctx.accounts.game.state = GameState::PlayerOneTurn;
        
        // Check if it was a legal starting move and in case save it
        require_eq!(board.iter().filter(|&x| x == &2u8).count(), 1, GameError::IllegalMove);
        ctx.accounts.game.board = board;

        // Save the player_two
        ctx.accounts.game.player_two = ctx.accounts.player_two.key();

        // Save the last update
        ctx.accounts.game.last_update = Clock::get()?.unix_timestamp;

        // Match the bet
        transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.player_two.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                }
            ),
            1 * LAMPORTS_PER_SOL,
        )?;
        
        Ok(())
    }

    pub fn play_game(ctx: Context<PlayGame>, board: [u8; 9]) -> Result<()> {

        // Check if it's not passed too much time
        require_gte!(Clock::get()?.unix_timestamp, ctx.accounts.game.last_update + DAY_IN_SECONDS, GameError::Timeout);

        // Check if the player is doing anything illegal
        match ctx.accounts.game.state {
            GameState::PlayerOneTurn => {
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_one, GameError::NotYourTurn);
                let current_player_one_moves = ctx.accounts.game.board.iter().filter(|&x| x == &1u8).count();
                let board_player_one_moves = board.iter().filter(|&x| x == &1u8).count();
                require_eq!(current_player_one_moves + 1, board_player_one_moves, GameError::IllegalMove);
                ctx.accounts.game.state = GameState::PlayerTwoTurn;
            }
            GameState::PlayerTwoTurn => {
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_two, GameError::NotYourTurn);
                let current_player_two_moves = ctx.accounts.game.board.iter().filter(|&x| x == &2u8).count();
                let board_player_two_moves = board.iter().filter(|&x| x == &2u8).count();
                require_eq!(current_player_two_moves + 1, board_player_two_moves, GameError::IllegalMove);
                ctx.accounts.game.state = GameState::PlayerOneTurn;
            }
            _ => return Err(GameError::GameNotPlayable.into()),
        }

        // Check if somebody won
        if let Some(winner) = check_winner(&board) {
            ctx.accounts.game.state = winner;
            
            match ctx.accounts.game.state {
                GameState::Draw => {},
                _ => {
                    // Pay the winner and close the game
                    let game_key = ctx.accounts.game.key();
                    let signer_seeds = &[b"vault".as_ref(), game_key.as_ref(), &[ctx.accounts.game.vault_bump]];
                    
                    transfer(
                        CpiContext::new_with_signer(
                            ctx.accounts.system_program.to_account_info(),
                            Transfer {
                                from: ctx.accounts.vault.to_account_info(),
                                to: ctx.accounts.player.to_account_info(),
                            },
                            &[signer_seeds]
                        ),
                        ctx.accounts.vault.lamports(),
                    )?;
                }
            }
        } else {
            ctx.accounts.game.board = board;
            ctx.accounts.game.last_update = Clock::get()?.unix_timestamp;
        }

        Ok(())
    }

    pub fn settle_game(ctx: Context<SettleGame>) -> Result<()> {

        let mut amount =  ctx.accounts.vault.lamports();
        
        match ctx.accounts.game.state {
            GameState::Pending => {
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_one, GameError::NotYourSettlment);
            },
            GameState::PlayerOneTurn => {
                require_gt!(ctx.accounts.game.last_update + DAY_IN_SECONDS, Clock::get()?.unix_timestamp, GameError::TimeNotPassed);
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_two, GameError::NotYourSettlment)
            },
            GameState::PlayerTwoTurn => {
                require_gt!(ctx.accounts.game.last_update + DAY_IN_SECONDS, Clock::get()?.unix_timestamp, GameError::TimeNotPassed);
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_one, GameError::NotYourSettlment)
            },
            GameState::Draw => {
                amount = amount.checked_div(2).unwrap();
                require!(ctx.accounts.player.key() == ctx.accounts.game.player_one || ctx.accounts.player.key() == ctx.accounts.game.player_two, GameError::NotYourSettlment);
                if ctx.accounts.player.key() == ctx.accounts.game.player_one {
                    ctx.accounts.game.state = GameState::PlayerOneClaimed;
                } else {
                    ctx.accounts.game.state = GameState::PlayerTwoClaimed;
                }
            },
            GameState::PlayerTwoClaimed => {
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_one, GameError::NotYourSettlment);
                ctx.accounts.game.state = GameState::DrawClaimed;
            },
            GameState::PlayerOneClaimed => {
                require_eq!(ctx.accounts.player.key(), ctx.accounts.game.player_two, GameError::NotYourSettlment);
                ctx.accounts.game.state = GameState::DrawClaimed;
            },
            _ => return Err(GameError::GameNotResolvable.into()),
        }

        // Pay the winner and close the game
        let game_key = ctx.accounts.game.key();
        let signer_seeds = &[b"vault".as_ref(), game_key.as_ref(), &[ctx.accounts.game.vault_bump]];
        
        transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: ctx.accounts.player.to_account_info(),
                },
                &[signer_seeds]
            ),
            amount,
        )?;

        Ok(())
    }
}

fn check_winner(board: &[u8; 9]) -> Option<GameState> {
    // Save all possible winning positions
    let winning_positions = [
        [0, 1, 2], [3, 4, 5], [6, 7, 8], // rows
        [0, 3, 6], [1, 4, 7], [2, 5, 8], // columns
        [0, 4, 8], [2, 4, 6],            // diagonals
    ];

    // Check if somebody won
    for &positions in winning_positions.iter() {
        let [a, b, c] = positions;
        if board[a] != 0 && board[a] == board[b] && board[a] == board[c] {
            return Some(if board[a] == 1 { GameState::PlayerOneWon } else { GameState::PlayerTwoWon });
        }
    }

    // Check if it's a draw
    if board.iter().all(|&x| x != 0) {
        Some(GameState::Draw)
    } else {
        None
    }
}

#[derive(Accounts)]
pub struct CreateGame<'info>{
    pub player: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        init, 
        payer = payer, 
        space = Game::INIT_SPACE,
        seeds = [b"game".as_ref(), player.key().as_ref()],
        bump,
    )]
    pub game: Account<'info, Game>,
    #[account(
        seeds = [b"vault".as_ref(), game.key().as_ref()],
        bump,
    )]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AcceptGame<'info>{
    pub player_two: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [b"game".as_ref(), game.player_one.key().as_ref()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
    #[account(
        seeds = [b"vault".as_ref(), game.key().as_ref()],
        bump = game.vault_bump, 
    )]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PlayGame<'info>{
    pub player: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [b"game".as_ref(), game.player_one.key().as_ref()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
    #[account(
        seeds = [b"vault".as_ref(), game.key().as_ref()],
        bump = game.vault_bump, 
    )]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SettleGame<'info>{
    pub player: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        close = payer,
        seeds = [b"game".as_ref(), game.player_one.key().as_ref()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
    #[account(
        seeds = [b"vault".as_ref(), game.key().as_ref()],
        bump = game.vault_bump, 
    )]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[account]
pub struct Game {
    pub state: GameState,
    pub board: [u8; 9],
    pub player_one: Pubkey,
    pub player_two: Pubkey,
    pub last_update: i64,
    pub bump: u8,
    pub vault_bump: u8,
}

impl Space for Game {
    const INIT_SPACE: usize = 8 + GameState::INIT_SPACE + 32 + 32 + 8 + 1;
}

#[derive(AnchorDeserialize, AnchorSerialize, Clone, InitSpace)]
pub enum GameState {
    Pending,
    PlayerOneTurn,
    PlayerTwoTurn,
    PlayerOneWon,
    PlayerTwoWon,
    Draw,
    PlayerOneClaimed,
    PlayerTwoClaimed,
    DrawClaimed,
}

#[error_code]
pub enum GameError {
    #[msg("The game has already started")]
    AlreadyStarted,
    #[msg("You cannot play against yourself")]
    CannotPlayAgainstYourself,
    #[msg("This move is Illegal")]
    IllegalMove,
    #[msg("Game has already finished or not started yet")]
    GameNotPlayable,
    #[msg("It's not your turn")]
    NotYourTurn,
    #[msg("Timeout")]
    Timeout,
    #[msg("Game is not resolvable yet")]
    GameNotResolvable,
    #[msg("It's not your settlement")]
    NotYourSettlment,
    #[msg("Time has not passed yet")]
    TimeNotPassed,
}

pub const DAY_IN_SECONDS: i64 = 60 * 60 * 24;