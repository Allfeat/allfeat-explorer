// Shared nav item shape for <AppHeader> and <MobileDrawer>. The header
// defines the single source of truth (ALL_NAV_ITEMS) and passes the
// filtered list to the drawer as a prop — this module keeps the drawer's
// prop type aligned with the header's output without either file
// redeclaring the same interface.

export type NavId =
  | 'overview'
  | 'ats'
  | 'token'
  | 'blocks'
  | 'extrinsics'
  | 'accounts'
  | 'events'
  | 'runtime'

/** Breakpoint at which the item moves into the `More` dropdown. `null` = always visible. */
export type CollapseAt = 'xxl' | 'xl' | null

export interface NavItem {
  id: NavId
  label: string
  path: string
  collapseAt: CollapseAt
  disabled?: boolean
  /** Hide this entry unless the active network exposes the feature. */
  mainnetOnly?: boolean
}
