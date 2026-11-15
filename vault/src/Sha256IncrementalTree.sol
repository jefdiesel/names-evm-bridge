// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

/// Append-only SHA-256 Merkle tree of depth 32 (same zero hashes as the Ethereum deposit
/// contract). SHA-256 is an EVM precompile and a RISC Zero accelerated hash, so both the
/// vault and the guests can use it cheaply.
library Sha256IncrementalTree {
    uint256 internal constant DEPTH = 32;
    uint256 internal constant CAPACITY = 1 << DEPTH;

    /// zeros[i] = root of an empty subtree of height i (zeros[0] = 0x00..00).
    bytes internal constant ZEROS = hex"0000000000000000000000000000000000000000000000000000000000000000f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4bdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71c78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c536d98837f2dd165a55d5eeae91485954472d56f246df256bf3cae19352a123c9efde052aa15429fae05bad4d0b1d7c64da64d03d7a1854a588c2cb8430c0d30d88ddfeed400a8755596b21942c1497e114c302e6118290f91e6772976041fa187eb0ddba57e35f6d286673802a4af5975e22506c7cf4c64bb6be5ee11527f2c26846476fd5fc54a5d43385167c95144f2643f533cc85bb9d16b782f8d7db193506d86582d252405b840018792cad2bf1259f1ef5aa5f887e13cb2f0094f51e1ffff0ad7e659772f9534c195c815efc4014ef1e1daed4404c06385d11192e92b6cf04127db05441cd833107a52be852868890e4317e6a02ab47683aa75964220b7d05f875f140027ef5118a2247bbb84ce8f2f0f1123623085daf7960c329f5fdf6af5f5bbdb6be9ef8aa618e4bf8073960867171e29676f8b284dea6a08a85eb58d900f5e182e3c50ef74969ea16c7726c549757cc23523c369587da7293784d49a7502ffcfb0340b1d7885688500ca308161a7f96b62df9d083b71fcc8f2bb8fe6b1689256c0d385f42f5bbe2027a22c1996e110ba97c171d3e5948de92beb8d0d63c39ebade8509e0ae3c9c3876fb5fa112be18f905ecacfecb92057603ab95eec8b2e541cad4e91de38385f2e046619f54496c2382cb6cacd5b98c26f5a4f893e908917775b62bff23294dbbe3a1cd8e6cc1c35b4801887b646a6f81f17fcddba7b592e3133393c16194fac7431abf2f5485ed711db282183c819e08ebaa8a8d7fe3af8caa085a7639a832001457dfb9128a8061142ad0335629ff23ff9cfeb3c337d7a51a6fbf00b9e34c52e1c9195c969bd4e7a0bfd51d5c5bed9c1167e71f0aa83cc32edfbefa9f4d3e0174ca85182eec9f3a09f6a6c0df6377a510d731206fa80a50bb6abe29085058f16212212a60eec8f049fecb92d8c8e0a84bc021352bfecbeddde993839f614c3dac0a3ee37543f9b412b16199dc158e23b544619e312724bb6d7c3153ed9de791d764a366b389af13c58bf8a8d90481a467657cdd2986268250628d0c10e385c58c6191e6fbe05191bcc04f133f2cea72c1c4848930bd7ba8cac54661072113fb278869e07bb8587f91392933374d017bcbe18869ff2c22b28cc10510d9853292803328be4fb0e80495e8bb8d271f5b889636b5fe28e79f1b850f8658246ce9b6a1e7b49fc06db7143e8fe0b4f2b0c5523a5c985e929f70af28d0bdd1a90a808f977f597c7c778c489e98d3bd8910d31ac0f7";

    error TreeFull();

    struct Tree {
        uint64 nextIndex;
        bytes32 root;
        bytes32[DEPTH] filled;
    }

    function zero(uint256 level) internal pure returns (bytes32 z) {
        bytes memory zs = ZEROS;
        assembly {
            z := mload(add(add(zs, 32), mul(level, 32)))
        }
    }

    function emptyRoot() internal pure returns (bytes32) {
        return sha256(abi.encodePacked(zero(DEPTH - 1), zero(DEPTH - 1)));
    }

    /// Ethereum deposit-contract algorithm: one storage write per insert (the branch node at
    /// the lowest level where the new count has a 1 bit), then the root from the branch.
    function insert(Tree storage t, bytes32 leaf) internal returns (uint64 index, bytes32 root) {
        index = t.nextIndex;
        if (index >= CAPACITY - 1) revert TreeFull();
        uint256 size = uint256(index) + 1;
        bytes32 node = leaf;
        for (uint256 h; h < DEPTH; ++h) {
            if (size & 1 == 1) {
                t.filled[h] = node;
                break;
            }
            node = sha256(abi.encodePacked(t.filled[h], node));
            size >>= 1;
        }
        bytes memory zs = ZEROS;
        size = uint256(index) + 1;
        node = bytes32(0);
        for (uint256 h; h < DEPTH; ++h) {
            if (size & 1 == 1) {
                node = sha256(abi.encodePacked(t.filled[h], node));
            } else {
                bytes32 z;
                assembly {
                    z := mload(add(add(zs, 32), mul(h, 32)))
                }
                node = sha256(abi.encodePacked(node, z));
            }
            size >>= 1;
        }
        t.root = node;
        t.nextIndex = uint64(index + 1);
        root = node;
    }
}
