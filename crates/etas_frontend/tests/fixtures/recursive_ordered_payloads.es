module library.api;
public spec Compare { flow compare(self, other: Self) -> i32 ![]; }
enum OrderedTree<K, V> { Empty, Branch(OrderedNode<K, V>) }
type OrderedNode<K, V> = { key: K, value: V, left: OrderedTree<K, V>, right: OrderedTree<K, V>, height: usize, count: usize };

impl OrderedTree<K, V> {
    flow height(self) -> usize ![] {
        return match self { OrderedTree.Empty => 0, OrderedTree.Branch(node) => node.height };
    }
    flow count(self) -> usize ![] {
        return match self { OrderedTree.Empty => 0, OrderedTree.Branch(node) => node.count };
    }
    flow branch(key: K, value: V, left: OrderedTree<K, V>, right: OrderedTree<K, V>) -> OrderedTree<K, V> ![] {
        let lh = left.height(); let rh = right.height();
        return OrderedTree.Branch(OrderedNode { key= key, value= value, left= left, right= right,
            height= 1 + if lh > rh { lh } else { rh }, count= 1 + left.count() + right.count() });
    }
    flow rotate_left(self) -> OrderedTree<K, V> ![] {
        let root = match self { OrderedTree.Branch(node) => node, OrderedTree.Empty => std.core.abort("AVL rotation invariant") };
        let pivot = match root.right { OrderedTree.Branch(node) => node, OrderedTree.Empty => std.core.abort("AVL rotation invariant") };
        return OrderedTree.branch(pivot.key, pivot.value, OrderedTree.branch(root.key, root.value, root.left, pivot.left), pivot.right);
    }
    flow rotate_right(self) -> OrderedTree<K, V> ![] {
        let root = match self { OrderedTree.Branch(node) => node, OrderedTree.Empty => std.core.abort("AVL rotation invariant") };
        let pivot = match root.left { OrderedTree.Branch(node) => node, OrderedTree.Empty => std.core.abort("AVL rotation invariant") };
        return OrderedTree.branch(pivot.key, pivot.value, pivot.left, OrderedTree.branch(root.key, root.value, pivot.right, root.right));
    }
    flow balance(self) -> OrderedTree<K, V> ![] {
        return match self {
            OrderedTree.Empty => self,
            OrderedTree.Branch(node) => {
                if node.left.height() > node.right.height() + 1 {
                    let left = match node.left {
                        OrderedTree.Empty => std.core.abort("AVL balance invariant"),
                        OrderedTree.Branch(child) => if child.right.height() > child.left.height() { node.left.rotate_left() } else { node.left },
                    };
                    return OrderedTree.branch(node.key, node.value, left, node.right).rotate_right();
                }
                if node.right.height() > node.left.height() + 1 {
                    let right = match node.right {
                        OrderedTree.Empty => std.core.abort("AVL balance invariant"),
                        OrderedTree.Branch(child) => if child.left.height() > child.right.height() { node.right.rotate_right() } else { node.right },
                    };
                    return OrderedTree.branch(node.key, node.value, node.left, right).rotate_left();
                }
                return self;
            },
        };
    }
    flow insert(self, key: K, value: V, compare: (K, K) -> i32 ![]) -> (OrderedTree<K, V>, bool) ![] {
        return match self {
            OrderedTree.Empty => (OrderedTree.branch(key, value, OrderedTree.Empty, OrderedTree.Empty), true),
            OrderedTree.Branch(node) => {
                let order = compare(key, node.key);
                if order == 0 { return (OrderedTree.branch(node.key, value, node.left, node.right), false); }
                if order < 0 {
                    let (left, added) = node.left.insert(key, value, compare);
                    return (OrderedTree.branch(node.key, node.value, left, node.right).balance(), added);
                }
                let (right, added) = node.right.insert(key, value, compare);
                return (OrderedTree.branch(node.key, node.value, node.left, right).balance(), added);
            },
        };
    }
}
public type Ordered<K, V> = { private tree: OrderedTree<K, V>, private compare: (K, K) -> i32 ![] };
impl Ordered<K ~ Compare, V> {
    public flow from_entries(items: List<(K, V)>) -> Ordered<K, V> ![] {
        var tree: OrderedTree<K, V> = OrderedTree.Empty;
        let compare = (a: K, b: K) => a.compare(b);
        for (key, value) in items {
            let (next, _) = tree.insert(key, value, compare);
            tree = next;
        }
        return Ordered { tree= tree, compare= compare };
    }
}

public flow ordered<K ~ Compare, V>(items: List<(K, V)>) -> Ordered<K, V> ![] {
    return Ordered.from_entries(items);
}

public type Cursor<T> = { next: () -> Option<(T, Cursor<T>)> ![] };
impl Cursor<T> {
    flow map<U>(self, transform: (T) -> U ![]) -> Cursor<U> ![] {
        return Cursor { next= () => match (self.next)() {
            None => None,
            Some((value, tail)) => Some((transform(value), tail.map(transform))),
        } };
    }
}
impl OrderedTree<K, V> {
    flow cursor(self, ancestors: List<OrderedNode<K, V>>) -> Cursor<(K, V)> ![] {
        return Cursor { next= () => {
            var current = self;
            var stack = ancestors;
            while true {
                match current {
                    OrderedTree.Branch(node) => {
                        stack = node :: stack;
                        current = node.left;
                    },
                    OrderedTree.Empty => match stack {
                        [] => { return None; },
                        node :: tail => { return Some(((node.key, node.value), node.right.cursor(tail))); },
                    },
                };
            }
        } };
    }
}
impl Ordered<K, V> {
    public flow keys(self) -> Cursor<K> ![] {
        return self.tree.cursor([]).map((entry: (K, V)) => {
            let (key, _) = entry;
            return key;
        });
    }
    public flow values(self) -> Cursor<V> ![] {
        return self.tree.cursor([]).map((entry: (K, V)) => {
            let (_, value) = entry;
            return value;
        });
    }
}

type Ranked<K> = { key: K };
type CandidateQueue<T, P> = { value: T, priority: P, compare: (P, P) -> i32 ![] };
impl CandidateQueue<T, P> {
    flow new(value: T, priority: P, compare: (P, P) -> i32 ![]) -> CandidateQueue<T, P> ![] {
        return CandidateQueue { value= value, priority= priority, compare= compare };
    }
    flow choose(self, value: T, priority: P) -> T ![] {
        return if (self.compare)(self.priority, priority) >= 0 { self.value } else { value };
    }
}
type BoundedChoice<T, K> = { key: (T) -> K ![], compare: (K, K) -> i32 ![] };
impl BoundedChoice<T, K> {
    flow choose(self, first: T, second: T) -> T ![] {
        let compare = self.compare;
        let queue = CandidateQueue.new(first, Ranked { key= (self.key)(first) }, (a: Ranked<K>, b: Ranked<K>) => compare(a.key, b.key));
        return queue.choose(second, Ranked { key= (self.key)(second) });
    }
}
public flow choose<T ~ Compare>(first: T, second: T) -> T ![] {
    let selection = BoundedChoice { key= (value: T) => value, compare= (a: T, b: T) => a.compare(b) };
    return selection.choose(first, second);
}

public flow empty_values<T>() -> List<T> ![] {
    return [];
}
