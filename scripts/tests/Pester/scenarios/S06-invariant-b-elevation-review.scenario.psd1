# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S06-invariant-b-elevation-review'
    Description = 'Multi-dependent cascade exercising Invariant B end-to-end. User explicitly releases ''b'' as a non-breaking change. The cascade pulls ''a'' (which depends on ''b''), ''alpha'' (which depends on ''b''), and ''zeta'' (which depends on ''a'') into the release set, all at non-breaking. Because ''a'' ALSO has pre-existing source modifications AND its cascade-applied change type is below breaking, the plan-review surfaces ''a'' for elevation review. The user ignores the elevation; ''a'' stays at the cascade-applied 0.3.1.
    
Validates two contracts simultaneously: (1) cascade-only members with no modifications are NOT prompted (Invariant A — verified by ''alpha'' and ''zeta'' going through without a prompt), and (2) cascade members WITH modifications below the breaking ceiling ARE prompted (Invariant B).'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'zeta';  Version = '0.1.0'; Deps = @(@{ Name = 'a' }) }
                @{ Name = 'alpha'; Version = '0.2.0'; Deps = @(@{ Name = 'b' }) }
                @{ Name = 'a';     Version = '0.3.0'; Deps = @(@{ Name = 'b' }) }
                @{ Name = 'b';     Version = '0.4.0' }
            )
        }
    }

    History = @(
        @{ Op = 'ModifySource'; Package = 'a' }
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        Packages = @('b@nonbreaking')
        Answers  = @(
            # Invariant B: 'a' was cascade-pulled with a non-breaking change AND
            # has pre-existing modifications, so plan-review surfaces it for
            # elevation review. User answers '2' (ignore — keep the
            # cascade-applied change type).
            @{ Match = "Choose option for 'a'"; Reply = '2' }
        )
    }

    Expect = @{
        # b is user-source, released as non-breaking on 0.x.y → 0.4.1.
        # a is cascade-released as non-breaking on 0.x.y → 0.3.1 (user accepted).
        # alpha is cascade-released as non-breaking on 0.x.y → 0.2.1.
        # zeta is cascade-released as non-breaking on 0.x.y → 0.1.1.
        Released = @(
            @{ Package = 'b';     To = '0.4.1' }
            @{ Package = 'a';     To = '0.3.1' }
            @{ Package = 'alpha'; To = '0.2.1' }
            @{ Package = 'zeta';  To = '0.1.1' }
        )
        PromptsRaised = @(
            "Choose option for 'a'"
        )
        UnconsumedAnswers = @()
    }
}
