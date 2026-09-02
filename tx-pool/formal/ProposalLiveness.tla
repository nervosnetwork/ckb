-------------------------- MODULE ProposalLiveness --------------------------
EXTENDS Naturals, FiniteSets

CONSTANTS Transactions, Closest, Farthest, ProposalServiceBound

ASSUME /\ Transactions # {}
       /\ Transactions \subseteq Nat
       /\ Closest > 0
       /\ Closest <= Farthest
       /\ ProposalServiceBound > 0

VARIABLES age, committed, lastEligible, lastCommitted,
          rankClock, everProposed, reproposed,
          proposalWait, serviceCount

vars == <<age, committed, lastEligible, lastCommitted,
          rankClock, everProposed, reproposed,
          proposalWait, serviceCount>>

First(set) ==
    CHOOSE tx \in set : \A other \in set : tx <= other

Pending ==
    {tx \in Transactions \ committed : age[tx] = 0}

Eligible ==
    {tx \in Transactions \ committed : age[tx] \in Closest..Farthest}

LocalCommitOffer ==
    IF Eligible # {} THEN {First(Eligible)} ELSE {}

LocalProposalOffer(chosen) ==
    LET available == Pending \ chosen
    IN IF available # {} THEN {First(available)} ELSE {}

CanonicalCommit(commitServe) ==
    IF commitServe THEN LocalCommitOffer ELSE {}

CanonicalProposal(proposalServe, commitServe, chosen) ==
    IF proposalServe \/ commitServe THEN LocalProposalOffer(chosen) ELSE {}

NextAge(tx, chosen, proposed) ==
    IF tx \in committed \cup chosen
       THEN 0
       ELSE IF tx \in proposed
               THEN 1
               ELSE IF age[tx] = 0 \/ age[tx] = Farthest
                       THEN 0
                       ELSE age[tx] + 1

Init ==
    /\ age = [tx \in Transactions |-> 0]
    /\ committed = {}
    /\ lastEligible = {}
    /\ lastCommitted = {}
    /\ rankClock = 0
    /\ everProposed = {}
    /\ reproposed = {}
    /\ proposalWait = 0
    /\ serviceCount = 0

Mine(proposalServe, commitServe) ==
    LET chosen == CanonicalCommit(commitServe)
        proposed == CanonicalProposal(proposalServe, commitServe, chosen)
    IN /\ committed' = committed \cup chosen
       /\ age' = [tx \in Transactions |-> NextAge(tx, chosen, proposed)]
       /\ lastEligible' = Eligible
       /\ lastCommitted' = chosen
       /\ rankClock' =
              IF committed' = Transactions
                 THEN 0
                 ELSE IF Cardinality(committed') > Cardinality(committed)
                         THEN 0
                         ELSE rankClock + 1
       /\ reproposed' = reproposed \cup (proposed \cap everProposed)
       /\ everProposed' = everProposed \cup proposed
       /\ proposalWait' =
              IF proposalServe \/ commitServe \/ Pending = {}
                 THEN 0
                 ELSE proposalWait + 1
       /\ serviceCount' =
              IF proposalServe \/ commitServe
                 THEN IF serviceCount < 2 THEN serviceCount + 1 ELSE 2
                 ELSE serviceCount

ProposalServiceRequired ==
    Pending # {} /\ proposalWait + 1 >= ProposalServiceBound

CommitWindowHitRequired ==
    Eligible # {} /\ age[First(Eligible)] = Farthest

ServiceRequired == ProposalServiceRequired \/ CommitWindowHitRequired

WindowHitNext ==
    \E proposalServe, commitServe \in BOOLEAN:
        /\ (ProposalServiceRequired => (proposalServe \/ commitServe))
        /\ (CommitWindowHitRequired => commitServe)
        /\ Mine(proposalServe, commitServe)

WindowHitSpec ==
    /\ Init
    /\ [][WindowHitNext]_vars
    /\ WF_vars(WindowHitNext)

\* A canonical service event occurs whenever this transaction is outside its
\* commit window, but never while it is eligible. Service therefore recurs
\* forever and still phase-misses every finite window.
PhaseMissNext == Mine(Eligible = {}, FALSE)

PhaseMissSpec ==
    /\ Init
    /\ [][PhaseMissNext]_vars
    /\ WF_vars(PhaseMissNext)

TypeOK ==
    /\ age \in [Transactions -> 0..Farthest]
    /\ committed \subseteq Transactions
    /\ lastEligible \subseteq Transactions
    /\ lastCommitted \subseteq Transactions
    /\ rankClock \in Nat
    /\ everProposed \subseteq Transactions
    /\ reproposed \subseteq everProposed
    /\ proposalWait \in 0..ProposalServiceBound
    /\ serviceCount \in 0..2

CommitSafe == lastCommitted \subseteq lastEligible

StrictServiceRank ==
    committed = Transactions \/ rankClock < ProposalServiceBound + Farthest

AllEventuallyCommitted == <> (committed = Transactions)

\* This deliberately false invariant is the shortest executable witness that
\* qualitative recurring service does not imply a proposal-window hit.
NoQualitativeServicePhaseMiss ==
    ~(serviceCount = 2 /\ reproposed # {} /\ committed = {})

=============================================================================
