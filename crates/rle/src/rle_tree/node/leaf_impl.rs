use crate::{rle_tree::tree_trait::Position, HasLength};
use std::fmt::{Debug, Error, Formatter};

use super::*;

impl<'a, T: Rle, A: RleTreeTrait<T>> LeafNode<'a, T, A> {
    #[inline]
    pub fn new(bump: &'a Bump, parent: NonNull<InternalNode<'a, T, A>>) -> Self {
        Self {
            bump,
            parent,
            children: BumpVec::with_capacity_in(A::MAX_CHILDREN_NUM, bump),
            prev: None,
            next: None,
            cache: Default::default(),
            _pin: PhantomPinned,
            _a: PhantomData,
        }
    }

    #[inline]
    fn _split(&mut self) -> &'a mut Self {
        let mut ans = self.bump.alloc(Self::new(self.bump, self.parent));
        for child in self
            .children
            .drain(self.children.len() - A::MIN_CHILDREN_NUM..self.children.len())
        {
            ans.children.push(child);
        }

        ans.next = self.next;
        ans.prev = Some(NonNull::new(self).unwrap());
        self.next = Some(NonNull::new(&mut *ans).unwrap());
        ans
    }

    pub fn push_child(&mut self, value: T) -> Result<(), &'a mut Self> {
        if !self.children.is_empty() {
            let last = self.children.last_mut().unwrap();
            if last.is_mergable(&value, &()) {
                last.merge(&value, &());
                A::update_cache_leaf(self);
                return Ok(());
            }
        }

        if self.children.len() == A::MAX_CHILDREN_NUM {
            let ans = self._split();
            ans.push_child(value).unwrap();
            A::update_cache_leaf(self);
            A::update_cache_leaf(ans);
            return Err(ans);
        }

        self.children.push(self.bump.alloc(value));
        A::update_cache_leaf(self);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn check(&mut self) {
        assert!(self.children.len() <= A::MAX_CHILDREN_NUM);

        let cache = self.cache.clone();
        A::update_cache_leaf(self);
        assert_eq!(cache, self.cache);
    }

    fn _delete_start(&mut self, from: A::Int) -> (usize, Option<usize>) {
        let (index_from, relative_from, pos_from) = A::find_pos_leaf(self, from);
        if pos_from == Position::Start {
            (index_from, None)
        } else {
            (index_from + 1, Some(relative_from))
        }
    }

    fn _delete_end(&mut self, to: A::Int) -> (usize, Option<usize>) {
        let (index_to, relative_to, pos_to) = A::find_pos_leaf(self, to);
        if pos_to == Position::End {
            (index_to + 1, None)
        } else {
            (index_to, Some(relative_to))
        }
    }

    pub fn insert(&mut self, raw_index: A::Int, value: T) -> Result<(), &'a mut Self> {
        match self._insert(raw_index, value) {
            Ok(_) => {
                A::update_cache_leaf(self);
                Ok(())
            }
            Err(new) => {
                A::update_cache_leaf(self);
                A::update_cache_leaf(new);
                Err(new)
            }
        }
    }

    fn _insert(&mut self, raw_index: A::Int, value: T) -> Result<(), &'a mut Self> {
        if self.children.is_empty() {
            self.children.push(self.bump.alloc(value));
            return Ok(());
        }

        let (mut index, mut offset, _pos) = A::find_pos_leaf(self, raw_index);
        let prev = {
            if offset == 0 && index > 0 {
                Some(&mut self.children[index - 1])
            } else if offset == self.children[index].len() {
                index += 1;
                offset = 0;
                Some(&mut self.children[index - 1])
            } else {
                None
            }
        };

        if let Some(prev) = prev {
            // clean cut, should no split
            if prev.is_mergable(&value, &()) {
                prev.merge(&value, &());
                return Ok(());
            }
        }

        let clean_cut = offset == 0 || offset == self.children[index].len();
        if clean_cut {
            return self._insert_with_split(index, value);
        }

        // need to split child
        let a = self.children[index].slice(0, offset);
        let b = self.children[index].slice(offset, self.children[index].len());
        self.children[index] = self.bump.alloc(a);

        if self.children.len() >= A::MAX_CHILDREN_NUM - 1 {
            let ans = self._split();
            if index < self.children.len() {
                self.children.insert(index + 1, self.bump.alloc(value));
                self.children.insert(index + 2, self.bump.alloc(b));
                ans.children.insert(0, self.children.pop().unwrap());
            } else {
                ans.children
                    .insert(index - self.children.len() + 1, self.bump.alloc(value));
                ans.children
                    .insert(index - self.children.len() + 2, self.bump.alloc(b));
            }

            return Err(ans);
        }

        self.children.insert(index + 1, self.bump.alloc(b));
        self.children.insert(index + 1, self.bump.alloc(value));
        Ok(())
    }

    #[inline]
    pub fn next(&self) -> Option<&Self> {
        self.next.map(|p| unsafe { p.as_ref() })
    }

    #[inline]
    pub fn prev(&self) -> Option<&Self> {
        self.prev.map(|p| unsafe { p.as_ref() })
    }

    #[inline]
    pub fn children(&self) -> &[&'a mut T] {
        &self.children
    }
}

impl<'a, T: Rle, A: RleTreeTrait<T>> LeafNode<'a, T, A> {
    /// Delete may cause the children num increase, because splitting may happen
    ///
    pub(crate) fn delete(
        &mut self,
        start: Option<A::Int>,
        end: Option<A::Int>,
    ) -> Result<(), &'a mut Self> {
        let (del_start, del_relative_from) = start.map_or((0, None), |x| self._delete_start(x));
        let (del_end, del_relative_to) =
            end.map_or((self.children.len(), None), |x| self._delete_end(x));
        let mut handled = false;
        let mut result = Ok(());
        if let (Some(del_relative_from), Some(del_relative_to)) =
            (del_relative_from, del_relative_to)
        {
            if del_start - 1 == del_end {
                let end = &mut self.children[del_end];
                let (left, right) = (
                    end.slice(0, del_relative_from),
                    end.slice(del_relative_to, end.len()),
                );

                *end = self.bump.alloc(left);
                result = self._insert_with_split(del_end + 1, right);
                handled = true;
            }
        }

        if !handled {
            if let Some(del_relative_from) = del_relative_from {
                self.children[del_start - 1] = self
                    .bump
                    .alloc(self.children[del_start - 1].slice(0, del_relative_from));
            }
            if let Some(del_relative_to) = del_relative_to {
                let end = &mut self.children[del_end];
                *end = self.bump.alloc(end.slice(del_relative_to, end.len()));
            }
        }

        if del_start < del_end {
            for _ in self.children.drain(del_start..del_end) {}
        }

        A::update_cache_leaf(self);
        if let Err(new) = &mut result {
            A::update_cache_leaf(new);
        }

        result
    }

    fn _insert_with_split(&mut self, index: usize, value: T) -> Result<(), &'a mut Self> {
        if self.children.len() == A::MAX_CHILDREN_NUM {
            let ans = self._split();
            if index <= self.children.len() {
                self.children.insert(index, self.bump.alloc(value));
            } else {
                ans.children
                    .insert(index - self.children.len(), self.bump.alloc(value));
            }

            Err(ans)
        } else {
            self.children.insert(index, self.bump.alloc(value));
            Ok(())
        }
    }
}

impl<'a, T: Rle, A: RleTreeTrait<T>> HasLength for LeafNode<'a, T, A> {
    #[inline]
    fn len(&self) -> usize {
        A::len_leaf(self)
    }
}

impl<'a, T: Rle, A: RleTreeTrait<T>> Debug for LeafNode<'a, T, A> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        let mut debug_struct = f.debug_struct("LeafNode");
        debug_struct.field("children", &self.children);
        debug_struct.field("cache", &self.cache);
        debug_struct.finish()
    }
}
